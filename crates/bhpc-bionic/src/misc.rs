//! Process-level services: `errno`, the environment, system properties, Android logging, process
//! control (`abort`, `exit`, stack protector, `__cxa_atexit`), identity and `sysconf`.

use crate::guest::{cstr, rd32, wr32, wr_cstr};
use crate::layout::{errno, sc, PAGE_SIZE, PROP_VALUE_MAX};
use crate::state::{AtExit, EnvVar, LogRecord, State};
use crate::stdio::{format_guest, Args};
use crate::{lock, Impl, Shared};
use bhpc_cpu::{Cpu, Stop};
use std::io::Write;

macro_rules! args {
    ($c:expr; $($n:ident),+) => {
        let mut a = $c.args();
        $(let $n = a.u32($c);)+
    };
}

// ---------- errno ----------

/// Store `e` in the guest's `errno`.
pub fn set_errno(c: &mut Cpu, st: &Shared, e: u32) {
    let addr = crate::sched::errno_addr(st);
    // The cell lives in the guest heap, which is always mapped.
    let _ = c.mem().write_u32(addr, e);
}

/// The guest's current `errno`.
pub fn get_errno(c: &mut Cpu, st: &Shared) -> u32 {
    let addr = crate::sched::errno_addr(st);
    c.mem().read_u32(addr).unwrap_or(0)
}

/// Set `errno` and return -1.
pub fn fail(c: &mut Cpu, st: &Shared, e: u32) {
    set_errno(c, st, e);
    c.ret_u32(-1i32 as u32);
}

fn errno_addr(c: &mut Cpu, st: &Shared) {
    let a = crate::sched::errno_addr(st);
    c.ret_u32(a);
}

// ---------- environment ----------

fn alloc_str(s: &mut State, bytes: &[u8]) -> u32 {
    let p = s.heap.alloc(bytes.len() as u32 + 1);
    let mut v = bytes.to_vec();
    v.push(0);
    s.mem.write_bytes(p, &v).expect("guest heap exhausted");
    p
}

/// Set `name=val` (replacing an existing value only with `overwrite`).
pub(crate) fn env_set(s: &mut State, name: &[u8], val: &[u8], overwrite: bool) {
    let existing = s.env.iter().position(|e| e.name == name);
    if existing.is_some() && !overwrite {
        return;
    }
    let mut full = name.to_vec();
    full.push(b'=');
    full.extend_from_slice(val);
    let base = alloc_str(s, &full);
    match existing {
        Some(i) => {
            let old = std::mem::replace(&mut s.env[i].base, base);
            s.heap.free(old);
        }
        None => s.env.push(EnvVar { name: name.to_vec(), base }),
    }
    env_sync(s);
}

/// Remove `name` if present.
pub(crate) fn env_unset(s: &mut State, name: &[u8]) {
    if let Some(i) = s.env.iter().position(|e| e.name == name) {
        let e = s.env.remove(i);
        s.heap.free(e.base);
        env_sync(s);
    }
}

/// Rebuild the guest `environ` array from the host list.
pub(crate) fn env_sync(s: &mut State) {
    let arr = s.heap.alloc(4 * (s.env.len() as u32 + 1));
    for (i, e) in s.env.iter().enumerate() {
        s.mem.write_u32(arr + 4 * i as u32, e.base).expect("guest heap block");
    }
    s.mem.write_u32(arr + 4 * s.env.len() as u32, 0).expect("guest heap block");
    if s.environ_array != 0 {
        let old = s.environ_array;
        s.heap.free(old);
    }
    s.environ_array = arr;
    let var = s.data["environ"];
    s.mem.write_u32(var, arr).expect("environ variable");
}

fn getenv(c: &mut Cpu, st: &Shared) {
    let n = c.args().u32(c);
    let Some(name) = cstr(c, n) else { return };
    let s = lock(st);
    let r = s.env.iter().find(|e| e.name == name).map_or(0, |e| e.base + name.len() as u32 + 1);
    if s.vfs.trace {
        eprintln!("[env] {} -> {}", String::from_utf8_lossy(&name), if r == 0 { "unset" } else { "set" });
    }
    drop(s);
    c.ret_u32(r);
}

fn valid_env_name(n: &[u8]) -> bool {
    !n.is_empty() && !n.contains(&b'=')
}

fn setenv(c: &mut Cpu, st: &Shared) {
    args!(c; n, v, overwrite);
    let (Some(name), Some(val)) = (cstr(c, n), cstr(c, v)) else { return };
    if !valid_env_name(&name) {
        return fail(c, st, errno::EINVAL);
    }
    env_set(&mut lock(st), &name, &val, overwrite != 0);
    c.ret_u32(0);
}

/// Unlike POSIX the string is copied, not adopted.
fn putenv(c: &mut Cpu, st: &Shared) {
    let p = c.args().u32(c);
    let Some(s) = cstr(c, p) else { return };
    match s.iter().position(|b| *b == b'=') {
        Some(0) => fail(c, st, errno::EINVAL),
        Some(i) => {
            env_set(&mut lock(st), &s[..i], &s[i + 1..], true);
            c.ret_u32(0);
        }
        None => {
            env_unset(&mut lock(st), &s);
            c.ret_u32(0);
        }
    }
}

fn unsetenv(c: &mut Cpu, st: &Shared) {
    let n = c.args().u32(c);
    let Some(name) = cstr(c, n) else { return };
    if !valid_env_name(&name) {
        return fail(c, st, errno::EINVAL);
    }
    env_unset(&mut lock(st), &name);
    c.ret_u32(0);
}

// ---------- system properties ----------

fn system_property_get(c: &mut Cpu, st: &Shared) {
    args!(c; name, value);
    let Some(n) = cstr(c, name) else { return };
    let v = lock(st).props.get(String::from_utf8_lossy(&n).as_ref()).cloned().unwrap_or_default();
    let bytes = &v.as_bytes()[..v.len().min(PROP_VALUE_MAX as usize - 1)];
    if wr_cstr(c, value, bytes) {
        c.ret_u32(bytes.len() as u32);
    }
}

// ---------- Android logging ----------

fn emit_log(c: &mut Cpu, st: &Shared, prio: i32, tag: u32, msg: &[u8]) {
    let tag =
        if tag == 0 { String::new() } else { String::from_utf8_lossy(&cstr(c, tag).unwrap_or_default()).into_owned() };
    let rec = LogRecord { prio, tag, msg: String::from_utf8_lossy(msg).into_owned() };
    let mut guard = lock(st);
    let s = &mut *guard;
    match s.log.as_mut() {
        Some(sink) => sink(&rec),
        None => {
            let p = match prio {
                2 => 'V',
                3 => 'D',
                4 => 'I',
                5 => 'W',
                6 => 'E',
                7 => 'F',
                _ => '?',
            };
            let _ = writeln!(s.stderr, "{p}/{}: {}", rec.tag, rec.msg);
        }
    }
    drop(guard);
    c.ret_u32(1);
}

fn log_write(c: &mut Cpu, st: &Shared) {
    args!(c; prio, tag, text);
    let Some(t) = cstr(c, text) else { return };
    emit_log(c, st, prio as i32, tag, &t);
}

fn log_print(c: &mut Cpu, st: &Shared) {
    let mut a = c.args();
    let (prio, tag, f) = (a.u32(c), a.u32(c), a.u32(c));
    if let Some(m) = format_guest(c, st, f, Args::Regs(a)) {
        emit_log(c, st, prio as i32, tag, &m);
    }
}

fn log_vprint(c: &mut Cpu, st: &Shared) {
    args!(c; prio, tag, f, ap);
    if let Some(m) = format_guest(c, st, f, Args::Va(ap)) {
        emit_log(c, st, prio as i32, tag, &m);
    }
}

// ---------- process control ----------

fn abort(c: &mut Cpu, _: &Shared) {
    c.request_stop(Stop::Host("guest called abort()".into()));
}

fn exit(c: &mut Cpu, st: &Shared) {
    let code = c.args().u32(c) as i32;
    lock(st).exit_code = Some(code);
    c.request_stop(Stop::Host(format!("guest called exit({code})")));
}

fn stack_chk_fail(c: &mut Cpu, _: &Shared) {
    c.request_stop(Stop::Host("stack smashing detected (__stack_chk_fail)".into()));
}

/// `__assert2(file, line, function, expression)`.
fn assert2(c: &mut Cpu, _: &Shared) {
    args!(c; file, line, func, expr);
    let (Some(f), Some(n), Some(e)) = (cstr(c, file), cstr(c, func), cstr(c, expr)) else { return };
    c.request_stop(Stop::Host(format!(
        "assertion failed: {} ({}:{} in {})",
        String::from_utf8_lossy(&e),
        String::from_utf8_lossy(&f),
        line,
        String::from_utf8_lossy(&n)
    )));
}

/// Recorded only: the process never really exits, so destructors are not run.
fn cxa_atexit(c: &mut Cpu, st: &Shared) {
    args!(c; func, arg, dso);
    lock(st).atexit.push(AtExit { func, arg, dso });
    c.ret_u32(0);
}

fn cxa_finalize(_: &mut Cpu, _: &Shared) {}

/// `__cxa_begin_cleanup(ucb)`: the compiler-generated cleanup pad of an ARM EHABI function announces itself.
/// Unwinding itself runs in the guest's own unwinder, so all the host owes the pad is "true" (resume with
/// `_Unwind_Resume` / `__cxa_end_cleanup` afterwards).
fn cxa_begin_cleanup(c: &mut Cpu, _: &Shared) {
    c.ret_u32(1);
}

/// Size of an ARM EHABI `_Unwind_Control_Block`, and the distance from it back to the start of libc++abi's
/// 32-bit `__cxa_exception` (reference count, type, destructor, two handlers, next, two counts).
const UCB_SIZE: u32 = 88;
const EXCEPTION_HEADER: u32 = 32;
/// `ctm_failed` / `ctm_succeeded` results of `__cxa_type_match`.
const CTM_FAILED: u32 = 0;
const CTM_SUCCEEDED: u32 = 1;

/// `__cxa_type_match(ucb, catch_type, is_reference, thrown_ptr_p)`: does the C++ exception being unwound match a
/// `catch (T)` clause? Handles foreign exceptions (never match) and exact type matches (same `type_info` object
/// or same mangled name), storing the address of the thrown object in `*thrown_ptr_p`; a thrown pointer is
/// unwrapped. Base-class and pointer-conversion catches are not recognised.
fn cxa_type_match(c: &mut Cpu, _: &Shared) {
    args!(c; ucb, catch_type, _is_reference, thrown_ptr_p);
    let (Some(class0), Some(class1)) = (rd32(c, ucb), rd32(c, ucb + 4)) else { return };
    let native =
        matches!([class0.to_le_bytes(), class1.to_le_bytes()].concat().as_slice(), b"GNUCC++\0" | b"CLNGC++\0");
    if !native {
        return c.ret_u32(CTM_FAILED);
    }
    let Some(thrown_type) = rd32(c, ucb - EXCEPTION_HEADER + 4) else { return };
    let (Some(thrown_name), Some(catch_name)) = (type_name(c, thrown_type), type_name(c, catch_type)) else {
        return c.ret_u32(CTM_FAILED);
    };
    if thrown_type != catch_type && thrown_name != catch_name {
        return c.ret_u32(CTM_FAILED);
    }
    let object = ucb + UCB_SIZE;
    let adjusted = if thrown_name.starts_with(b"P") { rd32(c, object).unwrap_or(0) } else { object };
    if wr32(c, thrown_ptr_p, adjusted) {
        c.ret_u32(CTM_SUCCEEDED);
    }
}

/// The mangled name of a `std::type_info` (its second word points at it), if readable.
fn type_name(c: &mut Cpu, type_info: u32) -> Option<Vec<u8>> {
    let name = c.mem().read_u32(type_info.checked_add(4)?).ok()?;
    c.mem().read_cstr(name).ok().map(String::into_bytes)
}

/// `cacheflush(start, end, flags)`: invalidate translated code in the range.
fn cacheflush(c: &mut Cpu, _: &Shared) {
    args!(c; start, end, _flags);
    c.invalidate(start, end.saturating_sub(start));
    c.ret_u32(0);
}

// ---------- identity and limits ----------

fn getpid(c: &mut Cpu, st: &Shared) {
    let p = lock(st).pid;
    c.ret_u32(p);
}

fn getppid(c: &mut Cpu, _: &Shared) {
    c.ret_u32(1);
}

fn getuid(c: &mut Cpu, st: &Shared) {
    let u = lock(st).uid;
    c.ret_u32(u);
}

fn sysconf(c: &mut Cpu, st: &Shared) {
    let name = c.args().u32(c);
    let cpus = lock(st).cpus;
    let v = match name {
        sc::ARG_MAX => 131_072,
        sc::CHILD_MAX => 1024,
        sc::CLK_TCK => 100,
        sc::OPEN_MAX => 1024,
        sc::PAGESIZE | sc::PAGE_SIZE => PAGE_SIZE,
        sc::NPROCESSORS_CONF | sc::NPROCESSORS_ONLN => cpus,
        sc::PHYS_PAGES => 131_072,
        sc::AVPHYS_PAGES => 65_536,
        _ => return fail(c, st, errno::EINVAL),
    };
    c.ret_u32(v);
}

/// `SCHED_FIFO` / `SCHED_RR` have priorities 1..=99, everything else only 0.
fn sched_get_priority_max(c: &mut Cpu, _: &Shared) {
    let p = c.args().u32(c);
    c.ret_u32(if matches!(p, 1 | 2) { 99 } else { 0 });
}

fn sched_get_priority_min(c: &mut Cpu, _: &Shared) {
    let p = c.args().u32(c);
    c.ret_u32(matches!(p, 1 | 2) as u32);
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("__errno", errno_addr),
    ("getenv", getenv),
    ("setenv", setenv),
    ("putenv", putenv),
    ("unsetenv", unsetenv),
    ("__system_property_get", system_property_get),
    ("__android_log_write", log_write),
    ("__android_log_print", log_print),
    ("__android_log_vprint", log_vprint),
    ("abort", abort),
    ("exit", exit),
    ("_exit", exit),
    ("__stack_chk_fail", stack_chk_fail),
    ("__assert2", assert2),
    ("__cxa_atexit", cxa_atexit),
    ("__cxa_finalize", cxa_finalize),
    ("__cxa_begin_cleanup", cxa_begin_cleanup),
    ("__cxa_type_match", cxa_type_match),
    ("cacheflush", cacheflush),
    ("getpid", getpid),
    ("gettid", getpid),
    ("getppid", getppid),
    ("getuid", getuid),
    ("geteuid", getuid),
    ("getgid", getuid),
    ("getegid", getuid),
    ("sysconf", sysconf),
    ("sched_get_priority_max", sched_get_priority_max),
    ("sched_get_priority_min", sched_get_priority_min),
];

#[cfg(test)]
mod tests {
    use crate::tests_env::{collecting_sink, Env, SCRATCH};
    use bhpc_cpu::Stop;

    const A: u32 = SCRATCH;
    const B: u32 = SCRATCH + 0x100;
    const C: u32 = SCRATCH + 0x200;
    const BAD: u32 = 0x10;

    fn getenv(e: &mut Env, name: &str) -> Option<String> {
        e.put_str(C, name);
        match e.call("getenv", &[C]).r0 {
            0 => None,
            p => Some(e.get_str(p)),
        }
    }

    fn environ(e: &Env) -> Vec<String> {
        let m = e.mem();
        let mut p = m.read_u32(e.data("environ")).unwrap();
        let mut v = Vec::new();
        while m.read_u32(p).unwrap() != 0 {
            v.push(m.read_cstr(m.read_u32(p).unwrap()).unwrap());
            p += 4;
        }
        v
    }

    #[test]
    fn errno_cell() {
        let mut e = Env::new();
        let a = e.call("__errno", &[]).r0;
        assert_eq!(e.call("__errno", &[]).r0, a);
        e.mem().write_u32(a, 5).unwrap();
        assert_eq!(e.errno(), 5);
        e.cpu.mem().write_u32(a, 0).unwrap();
        // Helpers.
        let mut c = e.cpu;
        super::set_errno(&mut c, &e.st, 33);
        assert_eq!(super::get_errno(&mut c, &e.st), 33);
        super::fail(&mut c, &e.st, 7);
        assert_eq!(c.reg(0) as i32, -1);
        assert_eq!(super::get_errno(&mut c, &e.st), 7);
    }

    #[test]
    fn environment() {
        let mut e = Env::new();
        assert_eq!(getenv(&mut e, "ANDROID_ROOT").as_deref(), Some("/system"));
        assert_eq!(getenv(&mut e, "NOPE"), None);
        e.put_str(A, "FOO");
        e.put_str(B, "bar");
        assert_eq!(e.call("setenv", &[A, B, 1]).r0, 0);
        assert_eq!(getenv(&mut e, "FOO").as_deref(), Some("bar"));
        e.put_str(B, "baz");
        e.call("setenv", &[A, B, 0]);
        assert_eq!(getenv(&mut e, "FOO").as_deref(), Some("bar"), "no overwrite");
        e.call("setenv", &[A, B, 1]);
        assert_eq!(getenv(&mut e, "FOO").as_deref(), Some("baz"));
        assert!(environ(&e).contains(&"FOO=baz".to_string()));
        assert_eq!(environ(&e).len(), 4);
        // putenv
        e.put_str(A, "QUX=1=2");
        assert_eq!(e.call("putenv", &[A]).r0, 0);
        assert_eq!(getenv(&mut e, "QUX").as_deref(), Some("1=2"));
        e.put_str(A, "QUX");
        assert_eq!(e.call("putenv", &[A]).r0, 0, "a bare name removes the variable");
        assert_eq!(getenv(&mut e, "QUX"), None);
        e.put_str(A, "=x");
        assert_eq!(e.call("putenv", &[A]).r0 as i32, -1);
        assert_eq!(e.errno(), 22);
        // unsetenv
        e.put_str(A, "FOO");
        assert_eq!(e.call("unsetenv", &[A]).r0, 0);
        assert_eq!(getenv(&mut e, "FOO"), None);
        assert_eq!(e.call("unsetenv", &[A]).r0, 0, "removing a missing variable succeeds");
        e.put_str(A, "");
        assert_eq!(e.call("unsetenv", &[A]).r0 as i32, -1);
        e.put_str(A, "A=B");
        assert_eq!(e.call("unsetenv", &[A]).r0 as i32, -1);
        assert_eq!(e.call("setenv", &[A, B, 1]).r0 as i32, -1);
        e.put_str(A, "");
        assert_eq!(e.call("setenv", &[A, B, 1]).r0 as i32, -1);
        for f in ["getenv", "unsetenv", "putenv"] {
            assert!(matches!(e.try_call(f, &[BAD]), Err(Stop::Fault(_))), "{f}");
        }
        assert!(matches!(e.try_call("setenv", &[BAD, A, 1]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("setenv", &[A, BAD, 1]), Err(Stop::Fault(_))));
    }

    #[test]
    fn custom_initial_environment() {
        let e = Env::with(|c| c.env = vec![("A".into(), "1".into())]);
        assert_eq!(environ(&e), ["A=1"]);
        let e = Env::with(|c| c.env = vec![]);
        assert!(environ(&e).is_empty());
    }

    #[test]
    fn system_properties() {
        let mut e = Env::with(|c| c.props.push(("long.prop".into(), "x".repeat(200))));
        e.put_str(A, "ro.build.version.sdk");
        assert_eq!(e.call("__system_property_get", &[A, B]).r0, 2);
        assert_eq!(e.get_str(B), "19");
        e.put_str(A, "no.such.prop");
        assert_eq!(e.call("__system_property_get", &[A, B]).r0, 0);
        assert_eq!(e.get_str(B), "");
        e.put_str(A, "long.prop");
        assert_eq!(e.call("__system_property_get", &[A, B]).r0, 91);
        assert_eq!(e.get_str(B).len(), 91);
        assert!(matches!(e.try_call("__system_property_get", &[BAD, B]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("__system_property_get", &[A, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn android_log_default_sink_writes_to_stderr() {
        let mut e = Env::new();
        e.put_str(A, "MyTag");
        e.put_str(B, "hello");
        assert_eq!(e.call("__android_log_write", &[4, A, B]).r0, 1);
        e.put_str(B, "v=%d %s");
        e.put_str(C, "str");
        assert_eq!(e.call("__android_log_print", &[6, A, B, 9, C]).r0, 1);
        assert_eq!(e.call("__android_log_print", &[5, 0, B, 9, C]).r0, 1);
        for (p, ch) in [(2, 'V'), (3, 'D'), (7, 'F'), (0, '?')] {
            e.put_str(B, "m");
            e.call("__android_log_write", &[p, A, B]);
            assert!(e.err.text().contains(&format!("{ch}/MyTag: m\n")), "{ch}");
        }
        let t = e.err.text();
        assert!(t.starts_with("I/MyTag: hello\nE/MyTag: v=9 str\nW/: v=9 str\n"), "{t}");
        // %n inside a log format stops the guest.
        e.put_str(B, "%n");
        assert!(matches!(e.try_call("__android_log_print", &[4, A, B, SCRATCH + 0x600]), Err(Stop::Host(_))));
        assert!(matches!(e.try_call("__android_log_write", &[4, A, BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn android_log_custom_sink_and_vprint() {
        let (logs, sink) = collecting_sink();
        let mut e = Env::with(|c| c.log = Some(sink));
        e.put_str(A, "T");
        e.put_str(B, "n=%d");
        let ap = SCRATCH + 0x800;
        e.mem().write_u32(ap, 12).unwrap();
        assert_eq!(e.call("__android_log_vprint", &[3, A, B, ap]).r0, 1);
        e.put_str(B, "plain");
        e.call("__android_log_write", &[4, 0, B]);
        let got = logs.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].prio, got[0].tag.as_str(), got[0].msg.as_str()), (3, "T", "n=12"));
        assert_eq!((got[1].tag.as_str(), got[1].msg.as_str()), ("", "plain"));
        assert_eq!(e.err.text(), "");
        e.put_str(B, "%n");
        assert!(matches!(e.try_call("__android_log_vprint", &[3, A, B, ap]), Err(Stop::Host(_))));
    }

    #[test]
    fn process_control() {
        let mut e = Env::new();
        assert_eq!(e.try_call("abort", &[]), Err(Stop::Host("guest called abort()".into())));
        assert_eq!(e.try_call("exit", &[3]), Err(Stop::Host("guest called exit(3)".into())));
        assert_eq!(crate::lock(&e.st).exit_code, Some(3));
        assert_eq!(e.try_call("_exit", &[-1i32 as u32]), Err(Stop::Host("guest called exit(-1)".into())));
        assert_eq!(crate::lock(&e.st).exit_code, Some(-1));
        let r = e.try_call("__stack_chk_fail", &[]);
        assert!(matches!(r, Err(Stop::Host(ref m)) if m.contains("stack smashing")));
        e.put_str(A, "game.c");
        e.put_str(B, "update");
        e.put_str(C, "x > 0");
        let r = e.try_call("__assert2", &[A, 42, B, C]);
        assert_eq!(r, Err(Stop::Host("assertion failed: x > 0 (game.c:42 in update)".into())));
        for k in 0..4 {
            let mut args = [A, 1, B, C];
            args[k] = BAD;
            if k != 1 {
                assert!(matches!(e.try_call("__assert2", &args), Err(Stop::Fault(_))), "arg {k}");
            }
        }
    }

    #[test]
    fn atexit_is_recorded_not_run() {
        let mut e = Env::new();
        assert_eq!(e.call("__cxa_atexit", &[0x1000, 0x2000, 0x3000]).r0, 0);
        assert_eq!(e.call("__cxa_atexit", &[0x1004, 0, 0x3000]).r0, 0);
        e.call("__cxa_finalize", &[0x3000]);
        let s = crate::lock(&e.st);
        assert_eq!(s.atexit.len(), 2);
        assert_eq!(s.atexit[0], crate::AtExit { func: 0x1000, arg: 0x2000, dso: 0x3000 });
    }

    #[test]
    fn cacheflush_invalidates_translations() {
        let mut e = Env::new();
        let code = SCRATCH + 0x1000;
        let m = e.mem();
        // mov r0,#1; bx lr
        m.write_u32(code, 0xE3A0_0001).unwrap();
        m.write_u32(code + 4, 0xE12F_FF1E).unwrap();
        assert_eq!(e.cpu.call(code, &[]).unwrap().r0, 1);
        m.write_u32(code, 0xE3A0_0002).unwrap();
        assert_eq!(e.call("cacheflush", &[code, code + 8, 0]).r0, 0);
        assert_eq!(e.cpu.call(code, &[]).unwrap().r0, 2);
        assert_eq!(e.call("cacheflush", &[code + 8, code, 0]).r0, 0, "inverted ranges are empty");
    }

    #[test]
    fn identity_and_limits() {
        let mut e = Env::with(|c| {
            c.cpus = 6;
            c.uid = 10123;
        });
        assert_eq!(e.call("getpid", &[]).r0, 4242);
        assert_eq!(e.call("gettid", &[]).r0, 4242);
        assert_eq!(e.call("getppid", &[]).r0, 1);
        for f in ["getuid", "geteuid", "getgid", "getegid"] {
            assert_eq!(e.call(f, &[]).r0, 10123, "{f}");
        }
        for (name, want) in [
            (0, 131_072),
            (5, 1024),
            (6, 100),
            (0xb, 1024),
            (0x27, 4096),
            (0x28, 4096),
            (0x60, 6),
            (0x61, 6),
            (0x62, 131_072),
            (0x63, 65_536),
        ] {
            assert_eq!(e.call("sysconf", &[name]).r0, want, "sysconf({name:#x})");
        }
        assert_eq!(e.call("sysconf", &[0x7777]).r0 as i32, -1);
        assert_eq!(e.errno(), 22);
        assert_eq!(e.call("sched_get_priority_max", &[1]).r0, 99);
        assert_eq!(e.call("sched_get_priority_max", &[0]).r0, 0);
        assert_eq!(e.call("sched_get_priority_min", &[2]).r0, 1);
        assert_eq!(e.call("sched_get_priority_min", &[0]).r0, 0);
        let mut e = Env::new();
        assert_eq!(e.call("sysconf", &[0x61]).r0, 4);
        assert!(e.data("__page_size") != 0);
        assert_eq!(e.mem().read_u32(e.data("__page_size")).unwrap(), 4096);
        assert_eq!(e.mem().read_u32(e.data("timezone")).unwrap(), 0);
        let tzname = e.data("tzname");
        let p = e.mem().read_u32(tzname + 4).unwrap();
        assert_eq!(e.get_str(p), "UTC");
    }

    #[test]
    fn getenv_can_be_traced() {
        let mut e = Env::new();
        crate::lock(&e.st).vfs.trace = true;
        e.put_str(SCRATCH, "ANDROID_ROOT");
        assert_ne!(e.call("getenv", &[SCRATCH]).r0, 0);
        e.put_str(SCRATCH, "NO_SUCH_VARIABLE");
        assert_eq!(e.call("getenv", &[SCRATCH]).r0, 0);
    }

    #[test]
    fn arm_ehabi_helpers() {
        let mut e = Env::new();
        assert_eq!(e.call("__cxa_begin_cleanup", &[A]).r0, 1);
        // An exception object: UCB at B, type_info at C (name "i"), another type_info with the same name at C+0x20.
        let m = e.mem();
        let (ucb, ti, ti_same, ti_other, ti_ptr, out) = (B, C, C + 0x20, C + 0x40, C + 0x60, A + 0x80);
        m.write_bytes(ucb, b"GNUCC++\0").unwrap();
        for (t, name, at) in
            [(ti, "i", C + 0x100), (ti_same, "i", C + 0x110), (ti_other, "f", C + 0x120), (ti_ptr, "Pc", C + 0x130)]
        {
            m.write_cstr(at, name).unwrap();
            m.write_u32(t + 4, at).unwrap();
        }
        let set_thrown = |t: u32| m.write_u32(ucb - 32 + 4, t).unwrap();
        set_thrown(ti);
        for (catch, want) in [(ti, 1), (ti_same, 1), (ti_other, 0)] {
            m.write_u32(out, 0).unwrap();
            assert_eq!(e.call("__cxa_type_match", &[ucb, catch, 0, out]).r0, want);
            assert_eq!(m.read_u32(out).unwrap(), if want == 1 { ucb + 88 } else { 0 });
        }
        // A thrown pointer is unwrapped to the pointee address.
        set_thrown(ti_ptr);
        m.write_u32(ucb + 88, 0x1234).unwrap();
        assert_eq!(e.call("__cxa_type_match", &[ucb, ti_ptr, 0, out]).r0, 1);
        assert_eq!(m.read_u32(out).unwrap(), 0x1234);
        // Foreign exceptions, unreadable type_infos and unreadable names never match.
        assert_eq!(e.call("__cxa_type_match", &[ucb, 0, 0, out]).r0, 0);
        assert_eq!(e.call("__cxa_type_match", &[ucb, ti, 0, out]).r0, 0);
        m.write_u32(ti_other + 4, 0).unwrap();
        assert_eq!(e.call("__cxa_type_match", &[ucb, ti_other, 0, out]).r0, 0);
        m.write_u32(ucb, 0x4443_4241).unwrap();
        assert_eq!(e.call("__cxa_type_match", &[ucb, ti_ptr, 0, out]).r0, 0);
        m.write_bytes(ucb, b"CLNGC++\0").unwrap();
        assert_eq!(e.call("__cxa_type_match", &[ucb, ti_ptr, 0, out]).r0, 1);
        assert!(matches!(e.try_call("__cxa_type_match", &[BAD, ti, 0, out]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("__cxa_type_match", &[ucb, ti_ptr, 0, BAD]), Err(Stop::Fault(_))));
        assert!(matches!(e.try_call("__cxa_type_match", &[ucb, 0xFFFF_FFFE, 0, out]), Ok(r) if r.r0 == 0));
    }
}
