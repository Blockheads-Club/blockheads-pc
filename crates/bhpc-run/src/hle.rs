//! Tiny host "libc" for the M0 fixture: just enough of bionic for `hello.so` and libc++'s
//! exception machinery. Everything unknown becomes a thunk that stops the guest loudly.

use bhpc_cpu::{Cpu, Stop, ThunkFn};
use bhpc_mem::GuestHeap;
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// One loaded module's unwind table, for `__gnu_Unwind_Find_exidx`.
#[derive(Debug, Clone, Copy)]
pub struct ExidxRange {
    pub start: u32,
    pub size: u32,
    pub table: u32,
    pub count: u32,
}

/// State shared by all host functions.
pub struct State {
    pub heap: GuestHeap,
    pub out: Box<dyn Write + Send>,
    pub checks: Vec<(String, bool)>,
    pub exidx: Vec<ExidxRange>,
    keys: HashMap<u32, u32>,
    next_key: u32,
    once_return: u32,
    data_symbols: HashMap<String, u32>,
}

pub type Shared = Arc<Mutex<State>>;

/// Size of bionic's `FILE` (84 bytes) times the three standard streams.
const SF_SIZE: u32 = 3 * 84;

/// Create the shared state. `heap` must already be mapped.
pub fn new_state(mut heap: GuestHeap, out: Box<dyn Write + Send>) -> Shared {
    let mut data_symbols = HashMap::new();
    let sf = heap.calloc(1, SF_SIZE);
    data_symbols.insert("__sF".to_string(), sf);
    let guard = heap.calloc(1, 4);
    data_symbols.insert("__stack_chk_guard".to_string(), guard);
    Arc::new(Mutex::new(State {
        heap,
        out,
        checks: Vec::new(),
        exidx: Vec::new(),
        keys: HashMap::new(),
        next_key: 1,
        once_return: 0,
        data_symbols,
    }))
}

fn thunk<F>(f: F) -> ThunkFn
where
    F: FnMut(&mut Cpu) + Send + 'static,
{
    Box::new(f)
}

/// The built-in host function for `name`, if any.
fn builtin(name: &str, st: &Shared) -> Option<ThunkFn> {
    let st = st.clone();
    let mem = |c: &Cpu| c.mem().clone();
    Some(match name {
        "puts" => thunk(move |c| {
            let mut a = c.args();
            let p = a.u32(c);
            let s = c.mem().read_cstr(p).unwrap_or_default();
            let _ = writeln!(st.lock().unwrap().out, "{s}");
            c.ret_u32(1);
        }),
        "bhpc_check" => thunk(move |c| {
            let mut a = c.args();
            let (n, ok) = (a.u32(c), a.u32(c));
            let name = c.mem().read_cstr(n).unwrap_or_default();
            let mut s = st.lock().unwrap();
            let _ = writeln!(s.out, "  [{}] {name}", if ok != 0 { "ok" } else { "FAIL" });
            s.checks.push((name, ok != 0));
        }),
        "bhpc_hypot" => thunk(|c| {
            let mut a = c.args();
            let (x, y) = (a.f64(c), a.f64(c));
            c.ret_f64(x.hypot(y));
        }),
        "bhpc_mix" => thunk(|c| {
            let mut a = c.args();
            let v: Vec<u32> = (0..6).map(|_| a.u32(c)).collect();
            let sum = v.iter().fold(0u32, |s, x| s.wrapping_add(*x));
            c.ret_u64((v[5] as u64) << 32 | sum as u64);
        }),
        "bhpc_nop" => thunk(|_| {}),
        "malloc" => thunk(move |c| {
            let n = c.args().u32(c);
            c.ret_u32(st.lock().unwrap().heap.alloc(n));
        }),
        "calloc" => thunk(move |c| {
            let mut a = c.args();
            let (n, s) = (a.u32(c), a.u32(c));
            c.ret_u32(st.lock().unwrap().heap.calloc(n, s));
        }),
        "realloc" => thunk(move |c| {
            let mut a = c.args();
            let (p, n) = (a.u32(c), a.u32(c));
            c.ret_u32(st.lock().unwrap().heap.realloc(p, n));
        }),
        "free" => thunk(move |c| {
            let p = c.args().u32(c);
            st.lock().unwrap().heap.free(p);
        }),
        "posix_memalign" => thunk(move |c| {
            let mut a = c.args();
            let (out, align, size) = (a.u32(c), a.u32(c), a.u32(c));
            let p = st.lock().unwrap().heap.memalign(align, size);
            if p == 0 {
                c.ret_u32(12); // ENOMEM
            } else {
                let _ = c.mem().write_u32(out, p);
                c.ret_u32(0);
            }
        }),
        "memcpy" | "memmove" => thunk(move |c| {
            let mut a = c.args();
            let (d, s, n) = (a.u32(c), a.u32(c), a.u32(c));
            let m = mem(c);
            if let Ok(b) = m.read_bytes(s, n) {
                let _ = m.write_bytes(d, &b);
            }
            c.ret_u32(d);
        }),
        "memset" => thunk(move |c| {
            let mut a = c.args();
            let (d, v, n) = (a.u32(c), a.u32(c), a.u32(c));
            let _ = c.mem().write_bytes(d, &vec![v as u8; n as usize]);
            c.ret_u32(d);
        }),
        "memcmp" => thunk(move |c| {
            let mut a = c.args();
            let (x, y, n) = (a.u32(c), a.u32(c), a.u32(c));
            let m = mem(c);
            let (bx, by) = (m.read_bytes(x, n).unwrap_or_default(), m.read_bytes(y, n).unwrap_or_default());
            c.ret_u32(bx.iter().zip(&by).find(|(p, q)| p != q).map_or(0, |(p, q)| (*p as i32 - *q as i32) as u32));
        }),
        "memchr" => thunk(move |c| {
            let mut a = c.args();
            let (p, v, n) = (a.u32(c), a.u32(c), a.u32(c));
            let b = c.mem().read_bytes(p, n).unwrap_or_default();
            c.ret_u32(b.iter().position(|x| *x == v as u8).map_or(0, |i| p + i as u32));
        }),
        "strlen" => thunk(|c| {
            let p = c.args().u32(c);
            c.ret_u32(c.mem().read_cstr(p).map_or(0, |s| s.len() as u32));
        }),
        "strcmp" => thunk(|c| {
            let mut a = c.args();
            let (x, y) = (a.u32(c), a.u32(c));
            let (sx, sy) = (c.mem().read_cstr(x).unwrap_or_default(), c.mem().read_cstr(y).unwrap_or_default());
            c.ret_u32(sx.cmp(&sy) as i32 as u32);
        }),
        "abort" => thunk(|c| c.request_stop(Stop::Host("guest called abort()".into()))),
        "__cxa_atexit" | "__cxa_finalize" | "pthread_mutex_lock" | "pthread_mutex_unlock" => thunk(|c| c.ret_u32(0)),
        "pthread_key_create" => thunk(move |c| {
            let key_out = c.args().u32(c);
            let mut s = st.lock().unwrap();
            let k = s.next_key;
            s.next_key += 1;
            s.keys.insert(k, 0);
            let _ = c.mem().write_u32(key_out, k);
            c.ret_u32(0);
        }),
        "pthread_key_delete" => thunk(move |c| {
            let k = c.args().u32(c);
            st.lock().unwrap().keys.remove(&k);
            c.ret_u32(0);
        }),
        "pthread_getspecific" => thunk(move |c| {
            let k = c.args().u32(c);
            c.ret_u32(st.lock().unwrap().keys.get(&k).copied().unwrap_or(0));
        }),
        "pthread_setspecific" => thunk(move |c| {
            let mut a = c.args();
            let (k, v) = (a.u32(c), a.u32(c));
            let mut s = st.lock().unwrap();
            let ok = s.keys.contains_key(&k);
            if ok {
                s.keys.insert(k, v);
            }
            c.ret_u32(if ok { 0 } else { 22 }); // EINVAL for unknown keys
        }),
        "pthread_once" => thunk(move |c| {
            let mut a = c.args();
            let (once, init) = (a.u32(c), a.u32(c));
            if c.mem().read_u32(once).unwrap_or(1) != 0 {
                c.ret_u32(0);
                return;
            }
            let _ = c.mem().write_u32(once, 1);
            // Run `init` as a tail call whose return lands in the "once_return" thunk, which pops
            // the original return address again (a nested JIT run is not allowed).
            let ret = st.lock().unwrap().once_return;
            let sp = c.sp() - 8;
            let _ = c.mem().write_u32(sp, c.reg(12));
            c.set_sp(sp);
            c.set_reg(14, ret);
            c.tail_call(init);
        }),
        "__pthread_once_return" => thunk(|c| {
            let sp = c.sp();
            let orig = c.mem().read_u32(sp).unwrap_or(0);
            c.set_sp(sp + 8);
            c.ret_u32(0);
            c.tail_call(orig);
        }),
        "__gnu_Unwind_Find_exidx" | "dl_unwind_find_exidx" => thunk(move |c| {
            let mut a = c.args();
            let (pc, count_out) = (a.u32(c), a.u32(c));
            let found = st.lock().unwrap().exidx.iter().find(|r| pc >= r.start && pc - r.start < r.size).copied();
            match found {
                Some(r) => {
                    let _ = c.mem().write_u32(count_out, r.count);
                    c.ret_u32(r.table);
                }
                None => {
                    let _ = c.mem().write_u32(count_out, 0);
                    c.ret_u32(0);
                }
            }
        }),
        _ => return None,
    })
}

/// Resolves guest imports: data symbols, built-in host functions, then "unimplemented" traps.
pub struct Hle<'a> {
    cpu: &'a mut Cpu,
    st: Shared,
}

impl<'a> Hle<'a> {
    /// Prepare the helper thunks (the `pthread_once` continuation).
    pub fn new(cpu: &'a mut Cpu, st: Shared) -> Self {
        if let Some(f) = builtin("__pthread_once_return", &st) {
            let addr = cpu.register_thunk("__pthread_once_return", f).expect("thunk page has room");
            st.lock().unwrap().once_return = addr;
        }
        Hle { cpu, st }
    }
}

impl bhpc_loader::HostSymbols for Hle<'_> {
    fn resolve(&mut self, name: &str) -> Option<u32> {
        if let Some(a) = self.st.lock().unwrap().data_symbols.get(name) {
            return Some(*a);
        }
        if let Some(a) = self.cpu.thunk_addr(name) {
            return Some(a);
        }
        let f = builtin(name, &self.st).unwrap_or_else(|| {
            let n = name.to_string();
            thunk(move |c| c.request_stop(Stop::Host(format!("unimplemented import {n}"))))
        });
        self.cpu.register_thunk(name, f).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{new_env, HEAP_BASE};
    use bhpc_cpu::Cpu;
    use bhpc_loader::HostSymbols;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    const SCRATCH: u32 = 0x5000_0000;

    struct Env {
        cpu: Box<Cpu>,
        st: Shared,
        out: Buf,
    }

    impl Env {
        fn new() -> Env {
            let out = Buf::default();
            let (cpu, st) = new_env(Box::new(out.clone()), None).unwrap();
            cpu.mem().map(SCRATCH, 0x2000).unwrap();
            Env { cpu, st, out }
        }
        fn addr(&mut self, name: &str) -> u32 {
            Hle::new(&mut self.cpu, self.st.clone()).resolve(name).unwrap()
        }
        fn call(&mut self, name: &str, args: &[u32]) -> bhpc_cpu::CallResult {
            let a = self.addr(name);
            self.cpu.call(a, args).unwrap()
        }
        fn put_str(&self, at: u32, s: &str) {
            self.cpu.mem().write_cstr(at, s).unwrap();
        }
    }

    #[test]
    fn output_and_check_thunks() {
        let mut e = Env::new();
        e.put_str(SCRATCH, "hello");
        e.put_str(SCRATCH + 0x100, "case");
        assert_eq!(e.call("puts", &[SCRATCH]).r0, 1);
        e.call("bhpc_check", &[SCRATCH + 0x100, 1]);
        e.call("bhpc_check", &[SCRATCH + 0x100, 0]);
        assert_eq!(e.out.text(), "hello\n  [ok] case\n  [FAIL] case\n");
        assert_eq!(e.st.lock().unwrap().checks, vec![("case".to_string(), true), ("case".to_string(), false)]);
        e.call("bhpc_nop", &[]);
    }

    #[test]
    fn math_and_mix_thunks() {
        let mut e = Env::new();
        let (a, b) = (3.0f64.to_bits(), 4.0f64.to_bits());
        let r = e.call("bhpc_hypot", &[a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32]);
        assert_eq!(r.as_f64(), 5.0);
        let r = e.call("bhpc_mix", &[1, 2, 3, 4, 5, 6]);
        assert_eq!(r.as_u64(), 6 << 32 | 21);
    }

    #[test]
    fn allocator_thunks() {
        let mut e = Env::new();
        let p = e.call("malloc", &[24]).r0;
        assert!(p >= HEAP_BASE);
        e.cpu.mem().write_u32(p, 0xfeed).unwrap();
        let q = e.call("realloc", &[p, 4096]).r0;
        assert_eq!(e.cpu.mem().read_u32(q).unwrap(), 0xfeed);
        e.call("free", &[q]);
        let z = e.call("calloc", &[4, 4]).r0;
        assert_eq!(e.cpu.mem().read_bytes(z, 16).unwrap(), vec![0; 16]);
        e.call("posix_memalign", &[SCRATCH, 64, 100]);
        assert_eq!(e.cpu.mem().read_u32(SCRATCH).unwrap() % 64, 0);
        assert_eq!(e.call("posix_memalign", &[SCRATCH, 3, 100]).r0, 12, "ENOMEM for a bad alignment");
    }

    #[test]
    fn string_and_memory_thunks() {
        let mut e = Env::new();
        e.put_str(SCRATCH, "abcdef");
        e.put_str(SCRATCH + 0x100, "abcxef");
        assert_eq!(e.call("strlen", &[SCRATCH]).r0, 6);
        assert_eq!(e.call("strlen", &[0x10]).r0, 0, "unmapped strings read as empty");
        assert_eq!(e.call("strcmp", &[SCRATCH, SCRATCH]).r0, 0);
        assert_eq!(e.call("strcmp", &[SCRATCH, SCRATCH + 0x100]).r0 as i32, -1);
        assert_eq!(e.call("memcmp", &[SCRATCH, SCRATCH + 0x100, 3]).r0, 0);
        assert_eq!(e.call("memcmp", &[SCRATCH, SCRATCH + 0x100, 6]).r0 as i32, 'd' as i32 - 'x' as i32);
        assert_eq!(e.call("memchr", &[SCRATCH, 'd' as u32, 6]).r0, SCRATCH + 3);
        assert_eq!(e.call("memchr", &[SCRATCH, 'z' as u32, 6]).r0, 0);
        assert_eq!(e.call("memcpy", &[SCRATCH + 0x200, SCRATCH, 7]).r0, SCRATCH + 0x200);
        assert_eq!(e.cpu.mem().read_cstr(SCRATCH + 0x200).unwrap(), "abcdef");
        e.call("memmove", &[SCRATCH + 1, SCRATCH, 3]);
        assert_eq!(e.cpu.mem().read_cstr(SCRATCH).unwrap(), "aabcef");
        e.call("memset", &[SCRATCH, 'z' as u32, 2]);
        assert_eq!(e.cpu.mem().read_cstr(SCRATCH).unwrap(), "zzbcef");
        // Unmapped source: nothing is written and nothing panics.
        e.call("memcpy", &[SCRATCH, 0x10, 4]);
    }

    #[test]
    fn process_and_thread_stubs() {
        let mut e = Env::new();
        let a = e.addr("abort");
        assert_eq!(e.cpu.call(a, &[]), Err(Stop::Host("guest called abort()".into())));
        for n in ["__cxa_atexit", "__cxa_finalize", "pthread_mutex_lock", "pthread_mutex_unlock"] {
            assert_eq!(e.call(n, &[1, 2, 3]).r0, 0, "{n}");
        }
        assert_eq!(e.call("pthread_key_create", &[SCRATCH, 0]).r0, 0);
        let key = e.cpu.mem().read_u32(SCRATCH).unwrap();
        assert_eq!(e.call("pthread_getspecific", &[key]).r0, 0);
        assert_eq!(e.call("pthread_setspecific", &[key, 77]).r0, 0);
        assert_eq!(e.call("pthread_getspecific", &[key]).r0, 77);
        assert_eq!(e.call("pthread_key_delete", &[key]).r0, 0);
        assert_eq!(e.call("pthread_setspecific", &[key, 1]).r0, 22);
        assert_eq!(e.call("pthread_getspecific", &[key]).r0, 0);
    }

    #[test]
    fn pthread_once_runs_init_once_via_tail_call() {
        let mut e = Env::new();
        let code = SCRATCH + 0x400;
        let (flag, once) = (SCRATCH + 0x800, SCRATCH + 0x804);
        // init: *flag += 5 (mov r1,#5; ldr r2,[pc,#4]; str r1,[r2]; bx lr; .word flag)
        for (i, w) in [0xE3A0_1005u32, 0xE59F_2004, 0xE582_1000, 0xE12F_FF1E, flag].iter().enumerate() {
            e.cpu.mem().write_u32(code + 4 * i as u32, *w).unwrap();
        }
        assert_eq!(e.call("pthread_once", &[once, code]).r0, 0);
        assert_eq!(e.cpu.mem().read_u32(flag).unwrap(), 5);
        assert_eq!(e.call("pthread_once", &[once, code]).r0, 0);
        assert_eq!(e.cpu.mem().read_u32(flag).unwrap(), 5, "second call is a no-op");
    }

    #[test]
    fn unwind_table_lookup() {
        let mut e = Env::new();
        e.st.lock().unwrap().exidx.push(ExidxRange { start: 0x1000_0000, size: 0x1000, table: 0x1000_0800, count: 9 });
        let r = e.call("__gnu_Unwind_Find_exidx", &[0x1000_0010, SCRATCH]);
        assert_eq!((r.r0, e.cpu.mem().read_u32(SCRATCH).unwrap()), (0x1000_0800, 9));
        let r = e.call("dl_unwind_find_exidx", &[0x2000_0000, SCRATCH]);
        assert_eq!((r.r0, e.cpu.mem().read_u32(SCRATCH).unwrap()), (0, 0));
    }

    #[test]
    fn resolver_data_symbols_and_unimplemented_imports() {
        let mut e = Env::new();
        let sf = e.addr("__sF");
        assert!(sf >= HEAP_BASE);
        assert_eq!(e.addr("__sF"), sf);
        assert!(e.addr("__stack_chk_guard") >= HEAP_BASE);
        let m1 = e.addr("malloc");
        assert_eq!(e.addr("malloc"), m1, "thunks are created once");
        let f = e.addr("fprintf");
        assert_eq!(e.cpu.call(f, &[]), Err(Stop::Host("unimplemented import fprintf".into())));
    }

    #[test]
    fn thunk_page_exhaustion_makes_resolve_fail() {
        let mut e = Env::new();
        let mut h = Hle::new(&mut e.cpu, e.st.clone());
        let mut last = Some(0);
        for i in 0..bhpc_cpu::MAX_THUNKS {
            last = h.resolve(&format!("sym{i}"));
            if last.is_none() {
                break;
            }
        }
        assert_eq!(h.resolve("one-more"), None);
        let _ = last;
    }
}
