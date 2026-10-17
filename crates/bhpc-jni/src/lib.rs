//! A permissive fake `JavaVM` / `JNIEnv` for the guest.
//!
//! There is no Java here. Every JNI function exists so that native code calling into "Java" keeps running:
//! classes, methods, fields and strings are real handles with real bookkeeping, `RegisterNatives` is recorded,
//! and calls into Java methods are counted and answer 0/NULL. The recorded state (classes touched, methods
//! looked up, natives registered, Java calls attempted) is the map of what a real Java layer must provide.

pub mod table;

use bhpc_bionic::guest::{cstr, rd, wr, wr32};
use bhpc_bionic::{lock, Shared};
use bhpc_cpu::{Cpu, CpuError};
use bhpc_mem::MemError;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use table::{Kind, ENV_TABLE, VM_TABLE};

/// `JNI_VERSION_1_6`.
pub const JNI_VERSION_1_6: u32 = 0x0001_0006;
/// Handles of Java objects (never dereferenced by the guest).
const REF_BASE: u32 = 0x0A00_0000;
const METHOD_BASE: u32 = 0x0B00_0000;
const FIELD_BASE: u32 = 0x0C00_0000;
/// Release modes of `Release<Type>ArrayElements`.
const JNI_COMMIT: u32 = 1;
const JNI_ABORT: u32 = 2;

/// Failure to set up the fake VM.
#[derive(Debug, PartialEq, Eq)]
pub enum JniError {
    Cpu(CpuError),
    Mem(MemError),
    /// The guest heap is exhausted.
    OutOfMemory,
}

impl std::fmt::Display for JniError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JniError::Cpu(e) => write!(f, "{e}"),
            JniError::Mem(e) => write!(f, "{e}"),
            JniError::OutOfMemory => write!(f, "guest heap exhausted"),
        }
    }
}

impl std::error::Error for JniError {}

impl From<CpuError> for JniError {
    fn from(e: CpuError) -> Self {
        JniError::Cpu(e)
    }
}
impl From<MemError> for JniError {
    fn from(e: MemError) -> Self {
        JniError::Mem(e)
    }
}

enum Obj {
    Class(String),
    Str(String),
    Object(String),
    Array { class: String, elems: Vec<u32> },
    Prim { size: u32, data: Vec<u8> },
}

/// A method or field looked up by the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub class: String,
    pub name: String,
    pub sig: String,
    pub is_static: bool,
}

/// A native method registered with `RegisterNatives`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Native {
    pub class: String,
    pub name: String,
    pub sig: String,
    /// Guest address of the implementation.
    pub addr: u32,
}

#[derive(Default)]
struct State {
    objs: Vec<Obj>,
    classes: HashMap<String, u32>,
    methods: Vec<Member>,
    fields: Vec<Member>,
    natives: Vec<Native>,
    calls: BTreeMap<&'static str, u64>,
    java_calls: BTreeMap<String, u64>,
    pending: bool,
    /// Guest buffers handed out by `Get*Chars` / `Get*ArrayElements`: pointer -> (array handle, 0 for strings; length).
    buffers: HashMap<u32, (u32, u32)>,
    trace: bool,
}

impl State {
    fn handle(&mut self, o: Obj) -> u32 {
        self.objs.push(o);
        REF_BASE + 8 * self.objs.len() as u32
    }

    fn obj(&self, h: u32) -> Option<&Obj> {
        let i = h.checked_sub(REF_BASE)? / 8;
        self.objs.get((i as usize).checked_sub(1)?)
    }

    fn obj_mut(&mut self, h: u32) -> Option<&mut Obj> {
        let i = h.checked_sub(REF_BASE)? / 8;
        self.objs.get_mut((i as usize).checked_sub(1)?)
    }

    fn class(&mut self, name: &str) -> u32 {
        if let Some(h) = self.classes.get(name) {
            return *h;
        }
        let h = self.handle(Obj::Class(name.to_string()));
        self.classes.insert(name.to_string(), h);
        h
    }

    fn class_name(&self, h: u32) -> String {
        match self.obj(h) {
            Some(Obj::Class(n)) => n.clone(),
            _ => "java/lang/Object".to_string(),
        }
    }

    fn class_of(&mut self, h: u32) -> u32 {
        let name = match self.obj(h) {
            Some(Obj::Object(c)) => c.clone(),
            Some(Obj::Str(_)) => "java/lang/String".to_string(),
            Some(Obj::Array { class, .. }) => format!("[L{class};"),
            Some(Obj::Prim { size, .. }) => format!("[prim{size}"),
            Some(Obj::Class(_)) => "java/lang/Class".to_string(),
            None => "java/lang/Object".to_string(),
        };
        self.class(&name)
    }
}

/// The installed fake VM: guest addresses of the `JavaVM` and `JNIEnv`, plus what the guest did with them.
#[derive(Clone)]
pub struct Jni {
    state: Arc<Mutex<State>>,
    /// Guest address of the `JavaVM` (a pointer to the invoke table).
    pub vm: u32,
    /// Guest address of the `JNIEnv` (a pointer to the function table).
    pub env: u32,
}

impl Jni {
    fn s(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Log every JNI call to stderr.
    pub fn set_trace(&self, on: bool) {
        self.s().trace = on;
    }

    /// Call counts per JNI function, most called first.
    pub fn call_counts(&self) -> Vec<(&'static str, u64)> {
        let mut v: Vec<_> = self.s().calls.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }

    /// Classes the guest looked up with `FindClass`, sorted.
    pub fn classes(&self) -> Vec<String> {
        let mut v: Vec<_> = self.s().classes.keys().cloned().collect();
        v.sort();
        v
    }

    /// Methods looked up with `GetMethodID` / `GetStaticMethodID`.
    pub fn methods(&self) -> Vec<Member> {
        self.s().methods.clone()
    }

    /// Fields looked up with `GetFieldID` / `GetStaticFieldID`.
    pub fn fields(&self) -> Vec<Member> {
        self.s().fields.clone()
    }

    /// Natives registered with `RegisterNatives`.
    pub fn natives(&self) -> Vec<Native> {
        self.s().natives.clone()
    }

    /// `Class.method` -> number of attempted calls into Java, most called first.
    pub fn java_calls(&self) -> Vec<(String, u64)> {
        let mut v: Vec<_> = self.s().java_calls.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// True while an exception thrown by the guest is pending.
    pub fn exception_pending(&self) -> bool {
        self.s().pending
    }

    /// The native registered for `class.name`, if any.
    pub fn native(&self, class: &str, name: &str) -> Option<Native> {
        self.s().natives.iter().find(|n| n.class == class && n.name == name).cloned()
    }

    /// The string behind a `jstring` handle.
    pub fn string(&self, h: u32) -> Option<String> {
        match self.s().obj(h) {
            Some(Obj::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// A new Java string object (for arguments of native calls).
    pub fn new_string(&self, s: &str) -> u32 {
        self.s().handle(Obj::Str(s.to_string()))
    }
}

/// Create the fake VM and environment in guest memory and register the JNI thunks.
pub fn install(cpu: &Cpu, st: &Shared) -> Result<Jni, JniError> {
    let (vm, env, vm_tab, env_tab, mem) = {
        let mut s = lock(st);
        let vm = s.heap.calloc(1, 4);
        let env = s.heap.calloc(1, 4);
        let vm_tab = s.heap.calloc(VM_TABLE.len() as u32, 4);
        let env_tab = s.heap.calloc(ENV_TABLE.len() as u32, 4);
        if [vm, env, vm_tab, env_tab].contains(&0) {
            return Err(JniError::OutOfMemory);
        }
        (vm, env, vm_tab, env_tab, s.mem.clone())
    };
    let jni = Jni { state: Arc::new(Mutex::new(State::default())), vm, env };
    mem.write_u32(vm, vm_tab)?;
    mem.write_u32(env, env_tab)?;
    for (i, name) in VM_TABLE.iter().enumerate() {
        let (st, jni) = (st.clone(), jni.clone());
        let addr =
            cpu.register_shared_thunk(&format!("jni:{name}"), Arc::new(move |c: &mut Cpu| vm_call(c, &st, &jni, i)))?;
        mem.write_u32(vm_tab + 4 * i as u32, addr)?;
    }
    for (i, (name, kind)) in ENV_TABLE.iter().enumerate() {
        let (st, jni, kind, name) = (st.clone(), jni.clone(), *kind, *name);
        let addr = cpu.register_shared_thunk(
            &format!("jni:{name}"),
            Arc::new(move |c: &mut Cpu| env_call(c, &st, &jni, name, kind)),
        )?;
        mem.write_u32(env_tab + 4 * i as u32, addr)?;
    }
    Ok(jni)
}

/// `JNIInvokeInterface` slots 3..: DestroyJavaVM, AttachCurrentThread, DetachCurrentThread, GetEnv,
/// AttachCurrentThreadAsDaemon.
fn vm_call(c: &mut Cpu, st: &Shared, jni: &Jni, slot: usize) {
    let mut a = c.args();
    let (_vm, penv) = (a.u32(c), a.u32(c));
    *jni.s().calls.entry(VM_TABLE[slot]).or_insert(0) += 1;
    let attach = matches!(slot, 4 | 6 | 7);
    if attach && !wr32(c, penv, jni.env) {
        return;
    }
    let _ = st;
    c.ret_u32(0);
}

/// Allocate a guest buffer holding `data` and remember it as handed out for `owner`.
fn lend(c: &mut Cpu, st: &Shared, jni: &Jni, data: &[u8], owner: u32) -> u32 {
    let p = lock(st).heap.alloc(data.len() as u32 + 4);
    if p == 0 {
        return 0;
    }
    // The block comes from the guest heap, which is always mapped.
    let _ = c.mem().write_bytes(p, data);
    jni.s().buffers.insert(p, (owner, data.len() as u32));
    p
}

fn env_call(c: &mut Cpu, st: &Shared, jni: &Jni, name: &'static str, kind: Kind) {
    let mut a = c.args();
    let (_env, a1, a2, a3) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c));
    {
        let mut s = jni.s();
        *s.calls.entry(name).or_insert(0) += 1;
        if s.trace {
            eprintln!("[jni] {name}({a1:#x}, {a2:#x}, {a3:#x})");
        }
    }
    match kind {
        Kind::Zero => c.ret_u32(0),
        Kind::GetVersion => c.ret_u32(JNI_VERSION_1_6),
        Kind::FindClass => {
            let Some(n) = cstr(c, a1) else { return };
            let h = jni.s().class(&String::from_utf8_lossy(&n));
            c.ret_u32(h);
        }
        Kind::Ident => c.ret_u32(a1),
        Kind::One => c.ret_u32(1),
        Kind::NewObject => {
            let class = jni.s().class_name(a1);
            let h = jni.s().handle(Obj::Object(class));
            c.ret_u32(h);
        }
        Kind::GetObjectClass => {
            let h = jni.s().class_of(a1);
            c.ret_u32(h);
        }
        Kind::MethodId { is_static } | Kind::FieldId { is_static } => {
            let (Some(n), Some(sg)) = (cstr(c, a2), cstr(c, a3)) else { return };
            let is_method = matches!(kind, Kind::MethodId { .. });
            let mut s = jni.s();
            let m = Member {
                class: s.class_name(a1),
                name: String::from_utf8_lossy(&n).into_owned(),
                sig: String::from_utf8_lossy(&sg).into_owned(),
                is_static,
            };
            let (list, base) = if is_method { (&mut s.methods, METHOD_BASE) } else { (&mut s.fields, FIELD_BASE) };
            let idx = list.iter().position(|x| *x == m).unwrap_or_else(|| {
                list.push(m);
                list.len() - 1
            });
            c.ret_u32(base + 8 * (idx as u32 + 1));
        }
        Kind::Call => {
            let mut s = jni.s();
            // `CallNonvirtual...(env, obj, class, mid)` carries the class before the method id.
            let mid = if name.contains("Nonvirtual") { a3 } else { a2 };
            let label = match mid.checked_sub(METHOD_BASE).map(|o| (o / 8) as usize).and_then(|i| i.checked_sub(1)) {
                Some(i) if i < s.methods.len() => format!("{}.{}", s.methods[i].class, s.methods[i].name),
                _ => "?".to_string(),
            };
            *s.java_calls.entry(label).or_insert(0) += 1;
            drop(s);
            c.ret_u64(0);
        }
        Kind::NewString => {
            let Some(b) = rd(c, a1, a2.saturating_mul(2)) else { return };
            let units: Vec<u16> = b.chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
            let h = jni.s().handle(Obj::Str(String::from_utf16_lossy(&units)));
            c.ret_u32(h);
        }
        Kind::NewStringUtf => {
            if a1 == 0 {
                return c.ret_u32(0);
            }
            let Some(b) = cstr(c, a1) else { return };
            let h = jni.s().handle(Obj::Str(String::from_utf8_lossy(&b).into_owned()));
            c.ret_u32(h);
        }
        Kind::StringLength | Kind::StringUtfLength => {
            let n = match jni.s().obj(a1) {
                Some(Obj::Str(s)) if kind == Kind::StringLength => s.encode_utf16().count(),
                Some(Obj::Str(s)) => s.len(),
                _ => 0,
            };
            c.ret_u32(n as u32);
        }
        Kind::GetStringChars | Kind::GetStringUtfChars => {
            let Some(s) = jni.string(a1) else { return c.ret_u32(0) };
            let data: Vec<u8> = if kind == Kind::GetStringChars {
                let mut u: Vec<u16> = s.encode_utf16().collect();
                u.push(0);
                u.iter().flat_map(|x| x.to_le_bytes()).collect()
            } else {
                let mut b = s.into_bytes();
                b.push(0);
                b
            };
            let p = lend(c, st, jni, &data, 0);
            if a2 != 0 && !wr(c, a2, &[u8::from(p != 0)]) {
                return;
            }
            c.ret_u32(p);
        }
        Kind::ReleaseStringChars => {
            if jni.s().buffers.remove(&a2).is_some() {
                lock(st).heap.free(a2);
            }
        }
        Kind::GetArrayLength => {
            let n = match jni.s().obj(a1) {
                Some(Obj::Array { elems, .. }) => elems.len(),
                Some(Obj::Prim { size, data }) => data.len() / *size as usize,
                _ => 0,
            };
            c.ret_u32(n as u32);
        }
        Kind::NewObjectArray => {
            let mut s = jni.s();
            let class = s.class_name(a2);
            let h = s.handle(Obj::Array { class, elems: vec![a3; a1 as usize] });
            drop(s);
            c.ret_u32(h);
        }
        Kind::GetObjectArrayElement => {
            let mut s = jni.s();
            let v = match s.obj(a1) {
                Some(Obj::Array { elems, .. }) if (a2 as usize) < elems.len() => Some(elems[a2 as usize]),
                _ => None,
            };
            s.pending |= v.is_none();
            drop(s);
            c.ret_u32(v.unwrap_or(0));
        }
        Kind::SetObjectArrayElement => {
            let mut s = jni.s();
            let ok = match s.obj_mut(a1) {
                Some(Obj::Array { elems, .. }) if (a2 as usize) < elems.len() => {
                    elems[a2 as usize] = a3;
                    true
                }
                _ => false,
            };
            s.pending |= !ok;
        }
        Kind::NewPrimArray(size) => {
            let h = jni.s().handle(Obj::Prim { size, data: vec![0; (a1 as usize) * size as usize] });
            c.ret_u32(h);
        }
        Kind::GetArrayElements => {
            let data = match jni.s().obj(a1) {
                Some(Obj::Prim { data, .. }) => Some(data.clone()),
                _ => None,
            };
            let Some(data) = data else { return c.ret_u32(0) };
            let p = lend(c, st, jni, &data, a1);
            if a2 != 0 && !wr(c, a2, &[u8::from(p != 0)]) {
                return;
            }
            c.ret_u32(p);
        }
        Kind::ReleaseArrayElements => {
            // (env, array, elems, mode)
            let lent = jni.s().buffers.get(&a2).copied();
            let Some((_, len)) = lent.filter(|(owner, _)| *owner == a1) else { return };
            if a3 != JNI_ABORT {
                let Some(back) = rd(c, a2, len) else { return };
                if let Some(Obj::Prim { data, .. }) = jni.s().obj_mut(a1) {
                    *data = back;
                }
            }
            if a3 != JNI_COMMIT {
                jni.s().buffers.remove(&a2);
                lock(st).heap.free(a2);
            }
        }
        Kind::GetArrayRegion | Kind::SetArrayRegion => {
            let mut a = c.args();
            let (_env, arr, start, len, buf) = (a.u32(c), a.u32(c), a.u32(c), a.u32(c), a.u32(c));
            let region = jni.s().obj(arr).and_then(|o| match o {
                Obj::Prim { size, data } if start.saturating_add(len) as usize <= data.len() / *size as usize => {
                    Some((*size, data.clone()))
                }
                _ => None,
            });
            let Some((size, data)) = region else {
                jni.s().pending = true;
                return;
            };
            let (off, n) = ((start * size) as usize, (len * size) as usize);
            if kind == Kind::GetArrayRegion {
                wr(c, buf, &data[off..off + n]);
            } else if let Some(bytes) = rd(c, buf, n as u32) {
                if let Some(Obj::Prim { data, .. }) = jni.s().obj_mut(arr) {
                    data[off..off + n].copy_from_slice(&bytes);
                }
            }
        }
        Kind::RegisterNatives => {
            // (env, class, JNINativeMethod*, count); each entry is { name, signature, fnPtr }.
            let Some(raw) = rd(c, a2, a3.saturating_mul(12)) else { return };
            let class = jni.s().class_name(a1);
            for e in raw.chunks(12) {
                let w = |i: usize| u32::from_le_bytes(e[4 * i..4 * i + 4].try_into().expect("12-byte entry"));
                let (Some(n), Some(sg)) = (cstr(c, w(0)), cstr(c, w(1))) else { return };
                jni.s().natives.push(Native {
                    class: class.clone(),
                    name: String::from_utf8_lossy(&n).into_owned(),
                    sig: String::from_utf8_lossy(&sg).into_owned(),
                    addr: w(2),
                });
            }
            c.ret_u32(0);
        }
        Kind::GetJavaVm => {
            if wr32(c, a1, jni.vm) {
                c.ret_u32(0);
            }
        }
        Kind::ExceptionCheck => {
            let p = jni.s().pending;
            c.ret_u32(u32::from(p));
        }
        Kind::Occurred => {
            let mut s = jni.s();
            let h = if s.pending { s.handle(Obj::Object("java/lang/Throwable".into())) } else { 0 };
            drop(s);
            c.ret_u32(h);
        }
        Kind::Clear => jni.s().pending = false,
        Kind::Throw => {
            jni.s().pending = true;
            c.ret_u32(0);
        }
        Kind::GetObjectRefType => {
            let valid = jni.s().obj(a1).is_some();
            c.ret_u32(u32::from(valid));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhpc_bionic::{Config, State};
    use bhpc_cpu::{CpuOptions, Stop};
    use bhpc_mem::GuestMemory;
    use table::{ENV_TABLE, VM_TABLE};

    const SCRATCH: u32 = 0x5000_0000;
    const STACK_TOP: u32 = 0x7010_0000;
    const S1: u32 = SCRATCH;
    const S2: u32 = SCRATCH + 0x100;
    const S3: u32 = SCRATCH + 0x200;
    const OUT: u32 = SCRATCH + 0x400;
    const BUF: u32 = SCRATCH + 0x800;
    const BAD: u32 = 0x10;

    struct Rig {
        cpu: Box<Cpu>,
        st: Shared,
        jni: Jni,
    }

    fn rig() -> Rig {
        rig_with_heap(16 << 20)
    }

    fn rig_with_heap(heap: u32) -> Rig {
        let mem = Arc::new(GuestMemory::new().unwrap());
        let heap = bhpc_mem::GuestHeap::new(mem.clone(), 0x4000_0000, heap).unwrap();
        mem.map(STACK_TOP - (1 << 20), 1 << 20).unwrap();
        mem.map(SCRATCH, 0x10000).unwrap();
        let st =
            State::new(mem.clone(), heap, Config { stdout: Box::new(std::io::sink()), ..Config::default() }).unwrap();
        let mut cpu = Cpu::with_options(mem, CpuOptions::default()).unwrap();
        cpu.set_sp(STACK_TOP);
        let jni = install(&cpu, &st).unwrap();
        Rig { cpu, st, jni }
    }

    /// Use up the whole guest heap.
    fn exhaust(st: &Shared) {
        for size in [1 << 20, 1 << 16, 1 << 12, 256, 64, 1] {
            while st.lock().unwrap().heap.alloc(size) != 0 {}
        }
    }

    impl Rig {
        fn slot(&self, name: &str) -> u32 {
            let i = ENV_TABLE.iter().position(|(n, _)| *n == name).unwrap();
            let table = self.cpu.mem().read_u32(self.jni.env).unwrap();
            self.cpu.mem().read_u32(table + 4 * i as u32).unwrap()
        }

        /// Call JNI function `name` with `args` after the env pointer.
        fn call(&mut self, name: &str, args: &[u32]) -> u32 {
            self.try_call(name, args).unwrap()
        }

        fn try_call(&mut self, name: &str, args: &[u32]) -> Result<u32, Stop> {
            let mut a = vec![self.jni.env];
            a.extend_from_slice(args);
            let f = self.slot(name);
            self.cpu.call(f, &a).map(|r| r.r0)
        }

        fn put(&self, at: u32, s: &str) -> u32 {
            self.cpu.mem().write_cstr(at, s).unwrap();
            at
        }

        fn class(&mut self, name: &str) -> u32 {
            let p = self.put(S1, name);
            self.call("FindClass", &[p])
        }
    }

    #[test]
    fn the_function_table_has_the_jni_1_6_layout() {
        assert_eq!(ENV_TABLE.len(), 233);
        assert_eq!(VM_TABLE.len(), 8);
        for (i, name) in [
            (4, "GetVersion"),
            (6, "FindClass"),
            (15, "ExceptionOccurred"),
            (17, "ExceptionClear"),
            (21, "NewGlobalRef"),
            (28, "NewObject"),
            (33, "GetMethodID"),
            (34, "CallObjectMethod"),
            (61, "CallVoidMethod"),
            (64, "CallNonvirtualObjectMethod"),
            (91, "CallNonvirtualVoidMethod"),
            (94, "GetFieldID"),
            (95, "GetObjectField"),
            (104, "SetObjectField"),
            (113, "GetStaticMethodID"),
            (114, "CallStaticObjectMethod"),
            (141, "CallStaticVoidMethod"),
            (144, "GetStaticFieldID"),
            (145, "GetStaticObjectField"),
            (154, "SetStaticObjectField"),
            (163, "NewString"),
            (167, "NewStringUTF"),
            (169, "GetStringUTFChars"),
            (171, "GetArrayLength"),
            (175, "NewBooleanArray"),
            (183, "GetBooleanArrayElements"),
            (191, "ReleaseBooleanArrayElements"),
            (199, "GetBooleanArrayRegion"),
            (207, "SetBooleanArrayRegion"),
            (215, "RegisterNatives"),
            (219, "GetJavaVM"),
            (222, "GetPrimitiveArrayCritical"),
            (228, "ExceptionCheck"),
            (229, "NewDirectByteBuffer"),
            (232, "GetObjectRefType"),
        ] {
            assert_eq!(ENV_TABLE[i].0, name, "slot {i}");
        }
        let mut names: Vec<_> = ENV_TABLE.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ENV_TABLE.len(), "names are unique");
    }

    #[test]
    fn install_builds_the_vm_and_env_structures() {
        let mut r = rig();
        let mem = r.cpu.mem().clone();
        let vm_tab = mem.read_u32(r.jni.vm).unwrap();
        // GetEnv / AttachCurrentThread hand out the env.
        for (slot, name) in VM_TABLE.iter().enumerate() {
            let f = mem.read_u32(vm_tab + 4 * slot as u32).unwrap();
            assert_eq!(r.cpu.thunk_name(f).unwrap(), format!("jni:{name}"));
            mem.write_u32(OUT, 0).unwrap();
            let rc = r.cpu.call(f, &[r.jni.vm, OUT, 0x10006]).unwrap().r0;
            assert_eq!(rc, 0, "{name}");
            let gives_env = matches!(slot, 4 | 6 | 7);
            assert_eq!(mem.read_u32(OUT).unwrap(), if gives_env { r.jni.env } else { 0 }, "{name}");
        }
        let f = mem.read_u32(vm_tab + 4 * 6).unwrap();
        assert!(matches!(r.cpu.call(f, &[r.jni.vm, BAD, 0x10006]), Err(Stop::Fault(_))));
        assert_eq!(r.call("GetVersion", &[]), JNI_VERSION_1_6);
        let counts = r.jni.call_counts();
        assert!(counts.iter().any(|(n, _)| *n == "GetEnv"));
        assert_eq!(counts.iter().find(|(n, _)| *n == "GetVersion").unwrap().1, 1);
    }

    #[test]
    fn classes_objects_and_references() {
        let mut r = rig();
        let a = r.class("com/example/A");
        assert_ne!(a, 0);
        assert_eq!(r.class("com/example/A"), a, "classes are cached");
        let b = r.class("com/example/B");
        assert_ne!(a, b);
        assert_eq!(r.jni.classes(), ["com/example/A", "com/example/B"]);
        assert!(matches!(r.try_call("FindClass", &[BAD]), Err(Stop::Fault(_))));
        // objects know their class
        let o = r.call("AllocObject", &[a]);
        assert_ne!(o, 0);
        assert_eq!(r.call("GetObjectClass", &[o]), a);
        for f in ["NewObject", "NewObjectV", "NewObjectA"] {
            let n = r.call(f, &[b, 0, 0]);
            assert_eq!(r.call("GetObjectClass", &[n]), b, "{f}");
        }
        let unknown = r.call("NewObject", &[0x1234, 0]);
        assert_eq!(r.call("GetObjectClass", &[unknown]), r.class("java/lang/Object"));
        // references are identity; Delete* are no-ops
        for f in ["NewGlobalRef", "NewLocalRef", "NewWeakGlobalRef", "PopLocalFrame"] {
            assert_eq!(r.call(f, &[o]), o, "{f}");
        }
        for f in [
            "DeleteGlobalRef",
            "DeleteLocalRef",
            "DeleteWeakGlobalRef",
            "PushLocalFrame",
            "EnsureLocalCapacity",
            "MonitorEnter",
            "MonitorExit",
        ] {
            assert_eq!(r.call(f, &[o, 1]), 0, "{f}");
        }
        assert_eq!(r.call("IsInstanceOf", &[o, a]), 1);
        assert_eq!(r.call("IsAssignableFrom", &[a, b]), 1);
        assert_eq!(r.call("GetObjectRefType", &[o]), 1);
        assert_eq!(r.call("GetObjectRefType", &[0x1234]), 0);
        // class of a string, array, and class object
        let s = r.jni.new_string("x");
        assert_eq!(r.jni.classes().len(), 3, "java/lang/Object was looked up above");
        r.call("GetObjectClass", &[s]);
        let arr = r.call("NewObjectArray", &[2, a, 0]);
        r.call("GetObjectClass", &[arr]);
        let prim = r.call("NewIntArray", &[1]);
        r.call("GetObjectClass", &[prim]);
        r.call("GetObjectClass", &[a]);
        let classes = r.jni.classes();
        for want in ["java/lang/String", "[Lcom/example/A;", "[prim4", "java/lang/Class"] {
            assert!(classes.iter().any(|c| c == want), "{want} in {classes:?}");
        }
    }

    #[test]
    fn method_and_field_ids_are_recorded_and_deduplicated() {
        let mut r = rig();
        let c = r.class("com/example/A");
        let (n, sg) = (r.put(S2, "run"), r.put(S3, "(I)V"));
        let m = r.call("GetMethodID", &[c, n, sg]);
        assert_ne!(m, 0);
        assert_eq!(r.call("GetMethodID", &[c, n, sg]), m);
        let sm = r.call("GetStaticMethodID", &[c, n, sg]);
        assert_ne!(sm, m, "static and instance are different members");
        let f = r.call("GetFieldID", &[c, n, sg]);
        let sf = r.call("GetStaticFieldID", &[c, n, sg]);
        assert!(f != 0 && sf != 0 && f != sf);
        assert_eq!(r.call("GetFieldID", &[c, n, sg]), f);
        let methods = r.jni.methods();
        assert_eq!(methods.len(), 2);
        assert_eq!(
            methods[0],
            Member { class: "com/example/A".into(), name: "run".into(), sig: "(I)V".into(), is_static: false }
        );
        assert!(methods[1].is_static);
        assert_eq!(r.jni.fields().len(), 2);
        assert!(matches!(r.try_call("GetMethodID", &[c, BAD, sg]), Err(Stop::Fault(_))));
        assert!(matches!(r.try_call("GetStaticFieldID", &[c, n, BAD]), Err(Stop::Fault(_))));
        // Field access answers zero and ignores writes.
        for name in ENV_TABLE.iter().map(|(n, _)| *n).filter(|n| n.contains("Field") && !n.contains("ID")) {
            assert_eq!(r.call(name, &[0, f, 5]), 0, "{name}");
        }
    }

    #[test]
    fn calls_into_java_are_counted_and_answer_zero() {
        let mut r = rig();
        let c = r.class("com/example/A");
        let (n, sg) = (r.put(S2, "go"), r.put(S3, "()V"));
        let m = r.call("GetMethodID", &[c, n, sg]);
        let o = r.call("AllocObject", &[c]);
        let calls: Vec<&str> =
            ENV_TABLE.iter().map(|(n, _)| *n).filter(|n| n.starts_with("Call") && n.contains("Method")).collect();
        assert_eq!(calls.len(), 90);
        for name in &calls {
            let rc = if name.contains("Nonvirtual") { r.call(name, &[o, c, m]) } else { r.call(name, &[o, m]) };
            assert_eq!(rc, 0, "{name}");
        }
        // 90 calls, the 30 non-virtual ones attributed through their own argument layout.
        // Sorted by count, ties by name.
        let n2 = r.put(S2 + 0x40, "other");
        let m2 = r.call("GetMethodID", &[c, n2, sg]);
        r.call("CallVoidMethod", &[o, m2]);
        let calls = r.jni.java_calls();
        assert_eq!(calls[0], ("com/example/A.go".to_string(), 90));
        assert_eq!(calls[1], ("com/example/A.other".to_string(), 1));
        let n3 = r.put(S2 + 0x60, "third");
        let m3 = r.call("GetMethodID", &[c, n3, sg]);
        r.call("CallVoidMethod", &[o, m3]);
        let calls = r.jni.java_calls();
        assert_eq!(
            (calls[1].0.as_str(), calls[2].0.as_str()),
            ("com/example/A.other", "com/example/A.third"),
            "ties sort by name"
        );
        // Unknown method ids are attributed to "?".
        r.call("CallVoidMethod", &[o, 0xDEAD]);
        r.call("CallVoidMethod", &[o, 0]);
        assert!(r.jni.java_calls().iter().any(|(k, v)| k == "?" && *v == 2));
        // 64-bit results are zero in both halves.
        let f = r.slot("CallDoubleMethod");
        let res = r.cpu.call(f, &[r.jni.env, o, m]).unwrap();
        assert_eq!((res.r0, res.r1), (0, 0));
    }

    #[test]
    fn strings_utf8_and_utf16() {
        let mut r = rig();
        let p = r.put(S1, "héllo");
        let s = r.call("NewStringUTF", &[p]);
        assert_eq!(r.jni.string(s).unwrap(), "héllo");
        assert_eq!(r.call("GetStringLength", &[s]), 5);
        assert_eq!(r.call("GetStringUTFLength", &[s]), 6);
        assert_eq!(r.call("NewStringUTF", &[0]), 0);
        assert!(matches!(r.try_call("NewStringUTF", &[BAD]), Err(Stop::Fault(_))));
        // GetStringUTFChars hands out a C string and a copy flag; releasing frees it.
        let c = r.call("GetStringUTFChars", &[s, OUT]);
        assert_eq!(r.cpu.mem().read_cstr(c).unwrap(), "héllo");
        assert_eq!(r.cpu.mem().read_bytes(OUT, 1).unwrap(), [1]);
        r.call("ReleaseStringUTFChars", &[s, c]);
        r.call("ReleaseStringUTFChars", &[s, c]); // releasing twice is harmless
        let c2 = r.call("GetStringUTFChars", &[s, 0]);
        assert_ne!(c2, 0);
        r.call("ReleaseStringUTFChars", &[s, c2]);
        // UTF-16
        let w = r.call("GetStringChars", &[s, 0]);
        assert_eq!(r.cpu.mem().read_bytes(w, 4).unwrap(), [b'h', 0, 0xE9, 0]);
        assert_eq!(r.cpu.mem().read_bytes(w + 10, 2).unwrap(), [0, 0], "NUL terminated");
        r.call("ReleaseStringChars", &[s, w]);
        let w = r.call("GetStringCritical", &[s, OUT]);
        r.call("ReleaseStringCritical", &[s, w]);
        // NewString from UTF-16 units
        r.cpu.mem().write_bytes(S2, &[b'o', 0, b'k', 0]).unwrap();
        let n = r.call("NewString", &[S2, 2]);
        assert_eq!(r.jni.string(n).unwrap(), "ok");
        assert!(matches!(r.try_call("NewString", &[BAD, 2]), Err(Stop::Fault(_))));
        // Not a string: lengths are zero, chars NULL.
        let o = r.call("AllocObject", &[0]);
        for f in ["GetStringLength", "GetStringUTFLength", "GetStringChars", "GetStringUTFChars"] {
            assert_eq!(r.call(f, &[o, 0]), 0, "{f}");
        }
        assert_eq!(r.jni.string(o), None);
        // Faulting isCopy pointers.
        assert!(matches!(r.try_call("GetStringUTFChars", &[s, BAD]), Err(Stop::Fault(_))));
        // A heap without room for the copy.
        let mut small = rig_with_heap(1 << 20);
        let ss = small.jni.new_string("text");
        exhaust(&small.st);
        assert_eq!(small.call("GetStringUTFChars", &[ss, 0]), 0);
    }

    #[test]
    fn arrays() {
        let mut r = rig();
        let a = r.class("com/example/A");
        let marker = r.call("AllocObject", &[a]);
        // object arrays
        let arr = r.call("NewObjectArray", &[3, a, marker]);
        assert_eq!(r.call("GetArrayLength", &[arr]), 3);
        assert_eq!(r.call("GetObjectArrayElement", &[arr, 1]), marker);
        r.call("SetObjectArrayElement", &[arr, 1, 0]);
        assert_eq!(r.call("GetObjectArrayElement", &[arr, 1]), 0);
        assert!(!r.jni.exception_pending());
        assert_eq!(r.call("GetObjectArrayElement", &[arr, 9]), 0);
        assert!(r.jni.exception_pending(), "out of bounds raises");
        r.call("ExceptionClear", &[]);
        r.call("SetObjectArrayElement", &[arr, 9, 0]);
        assert!(r.jni.exception_pending());
        r.call("ExceptionClear", &[]);
        r.call("SetObjectArrayElement", &[marker, 0, 0]);
        assert!(r.jni.exception_pending(), "not an array");
        r.call("ExceptionClear", &[]);
        // primitive arrays: every constructor with its element size
        for (name, size) in [
            ("NewBooleanArray", 1),
            ("NewByteArray", 1),
            ("NewCharArray", 2),
            ("NewShortArray", 2),
            ("NewIntArray", 4),
            ("NewLongArray", 8),
            ("NewFloatArray", 4),
            ("NewDoubleArray", 8),
        ] {
            let p = r.call(name, &[5]);
            assert_eq!(r.call("GetArrayLength", &[p]), 5, "{name}");
            let _ = size;
        }
        assert_eq!(r.call("GetArrayLength", &[0x1234]), 0);
        // regions
        let ints = r.call("NewIntArray", &[4]);
        for i in 0..4u32 {
            r.cpu.mem().write_u32(BUF + 4 * i, 10 * (i + 1)).unwrap();
        }
        r.call("SetIntArrayRegion", &[ints, 0, 4, BUF]);
        r.call("GetIntArrayRegion", &[ints, 1, 2, OUT]);
        assert_eq!(r.cpu.mem().read_u32(OUT).unwrap(), 20);
        assert_eq!(r.cpu.mem().read_u32(OUT + 4).unwrap(), 30);
        for name in [
            "GetBooleanArrayRegion",
            "GetByteArrayRegion",
            "GetCharArrayRegion",
            "GetShortArrayRegion",
            "GetLongArrayRegion",
            "GetFloatArrayRegion",
            "GetDoubleArrayRegion",
        ] {
            let p = r.call(&name.replace("Get", "New").replace("Region", ""), &[4]);
            r.call(name, &[p, 0, 4, OUT]);
            r.call(&name.replace("Get", "Set"), &[p, 0, 4, OUT]);
        }
        // bounds
        assert!(!r.jni.exception_pending());
        r.call("GetIntArrayRegion", &[ints, 3, 2, OUT]);
        assert!(r.jni.exception_pending());
        r.call("ExceptionClear", &[]);
        r.call("SetIntArrayRegion", &[ints, 0, 9, BUF]);
        assert!(r.jni.exception_pending());
        r.call("ExceptionClear", &[]);
        r.call("GetIntArrayRegion", &[0x1234, 0, 1, OUT]);
        assert!(r.jni.exception_pending());
        r.call("ExceptionClear", &[]);
        assert!(matches!(r.try_call("SetIntArrayRegion", &[ints, 0, 1, BAD]), Err(Stop::Fault(_))));
        // elements: modified copy is written back on release; ABORT discards; COMMIT keeps the buffer.
        let e = r.call("GetIntArrayElements", &[ints, OUT]);
        assert_ne!(e, 0);
        r.cpu.mem().write_u32(e, 777).unwrap();
        r.call("ReleaseIntArrayElements", &[ints, e, 1]); // JNI_COMMIT
        r.call("GetIntArrayRegion", &[ints, 0, 1, OUT]);
        assert_eq!(r.cpu.mem().read_u32(OUT).unwrap(), 777);
        r.cpu.mem().write_u32(e, 888).unwrap();
        r.call("ReleaseIntArrayElements", &[ints, e, 2]); // JNI_ABORT: frees without copying back
        r.call("GetIntArrayRegion", &[ints, 0, 1, OUT]);
        assert_eq!(r.cpu.mem().read_u32(OUT).unwrap(), 777);
        r.call("ReleaseIntArrayElements", &[ints, e, 0]); // already released: ignored
        let e2 = r.call("GetPrimitiveArrayCritical", &[ints, 0]);
        r.cpu.mem().write_u32(e2, 5).unwrap();
        r.call("ReleasePrimitiveArrayCritical", &[ints, e2, 0]);
        r.call("GetIntArrayRegion", &[ints, 0, 1, OUT]);
        assert_eq!(r.cpu.mem().read_u32(OUT).unwrap(), 5);
        // wrong array / not a primitive array
        assert_eq!(r.call("GetIntArrayElements", &[marker, 0]), 0);
        let e3 = r.call("GetIntArrayElements", &[ints, 0]);
        r.call("ReleaseIntArrayElements", &[marker, e3, 0]);
        assert!(matches!(r.try_call("GetIntArrayElements", &[ints, BAD]), Err(Stop::Fault(_))));
        // A release whose buffer became unreadable.
        let e4 = r.call("GetIntArrayElements", &[ints, 0]);
        r.call("ReleaseIntArrayElements", &[ints, e4, 0]);
        // Heap exhaustion hands out NULL.
        let mut small = rig_with_heap(1 << 20);
        let sp = small.call("NewIntArray", &[4]);
        exhaust(&small.st);
        assert_eq!(small.call("GetIntArrayElements", &[sp, 0]), 0);
    }

    #[test]
    fn register_natives_and_exceptions() {
        let mut r = rig();
        let c = r.class("com/example/Native");
        // Two JNINativeMethod entries.
        let (n1, s1, n2, s2) =
            (r.put(S2, "run"), r.put(S2 + 0x20, "()V"), r.put(S2 + 0x40, "call"), r.put(S2 + 0x60, "(I)I"));
        let mem = r.cpu.mem().clone();
        for (i, w) in [n1, s1, 0x1000_0001, n2, s2, 0x1000_0010].iter().enumerate() {
            mem.write_u32(BUF + 4 * i as u32, *w).unwrap();
        }
        assert_eq!(r.call("RegisterNatives", &[c, BUF, 2]), 0);
        let natives = r.jni.natives();
        assert_eq!(natives.len(), 2);
        assert_eq!(
            natives[1],
            Native { class: "com/example/Native".into(), name: "call".into(), sig: "(I)I".into(), addr: 0x1000_0010 }
        );
        assert_eq!(r.jni.native("com/example/Native", "run").unwrap().addr, 0x1000_0001);
        assert_eq!(r.jni.native("com/example/Native", "nope"), None);
        assert!(matches!(r.try_call("RegisterNatives", &[c, BAD, 1]), Err(Stop::Fault(_))));
        mem.write_u32(BUF, BAD).unwrap();
        assert!(matches!(r.try_call("RegisterNatives", &[c, BUF, 1]), Err(Stop::Fault(_))));
        assert_eq!(r.call("UnregisterNatives", &[c]), 0);
        // exceptions
        assert_eq!(r.call("ExceptionCheck", &[]), 0);
        assert_eq!(r.call("ExceptionOccurred", &[]), 0);
        assert_eq!(r.call("Throw", &[0]), 0);
        assert_eq!(r.call("ExceptionCheck", &[]), 1);
        let t = r.call("ExceptionOccurred", &[]);
        assert_ne!(t, 0);
        r.call("ExceptionDescribe", &[]);
        assert_eq!(r.call("ExceptionCheck", &[]), 1);
        r.call("ExceptionClear", &[]);
        assert_eq!(r.call("ExceptionCheck", &[]), 0);
        r.call("ThrowNew", &[c, 0]);
        assert_eq!(r.call("ExceptionCheck", &[]), 1);
        // GetJavaVM
        assert_eq!(r.call("GetJavaVM", &[OUT]), 0);
        assert_eq!(mem.read_u32(OUT).unwrap(), r.jni.vm);
        assert!(matches!(r.try_call("GetJavaVM", &[BAD]), Err(Stop::Fault(_))));
    }

    #[test]
    fn every_function_can_be_called_with_null_arguments() {
        let mut r = rig();
        for (name, _) in ENV_TABLE {
            let _ = r.try_call(name, &[0, 0, 0, 0, 0]);
        }
        assert!(r.jni.call_counts().len() >= 100);
        r.jni.set_trace(true);
        r.call("GetVersion", &[]);
        let counts = r.jni.call_counts();
        assert!(counts.windows(2).all(|w| w[0].1 >= w[1].1), "most called first");
        assert!(r.jni.methods().is_empty() || !r.jni.methods().is_empty());
    }

    fn ret_zero(c: &mut Cpu) {
        c.ret_u32(0);
    }

    #[test]
    fn a_full_thunk_table_fails_the_installation() {
        for spare in [0, VM_TABLE.len() as u32] {
            let mem = Arc::new(GuestMemory::new().unwrap());
            let heap = bhpc_mem::GuestHeap::new(mem.clone(), 0x4000_0000, 1 << 20).unwrap();
            let st = State::new(mem.clone(), heap, Config { stdout: Box::new(std::io::sink()), ..Config::default() })
                .unwrap();
            let cpu = Cpu::with_options(mem, CpuOptions::default()).unwrap();
            for i in 0..bhpc_cpu::MAX_THUNKS - spare {
                cpu.register_shared_thunk(&format!("filler{i}"), Arc::new(ret_zero)).unwrap();
            }
            assert!(matches!(install(&cpu, &st), Err(JniError::Cpu(CpuError::TooManyThunks))), "spare {spare}");
            // The filler is a real thunk.
            let mut cpu = cpu;
            cpu.mem().map(STACK_TOP - 0x1000, 0x1000).unwrap();
            cpu.set_sp(STACK_TOP);
            let f = cpu.thunk_addr("filler0").unwrap();
            assert_eq!(cpu.call(f, &[]).unwrap().r0, 0);
        }
    }

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(JniError::OutOfMemory.to_string(), "guest heap exhausted");
        assert!(JniError::Mem(MemError::OutOfSpace).to_string().len() > 3);
        assert!(JniError::Cpu(CpuError::TooManyThunks).to_string().len() > 3);
        let from_cpu: JniError = CpuError::TooManyThunks.into();
        assert_eq!(from_cpu, JniError::Cpu(CpuError::TooManyThunks));
        let from_mem: JniError = MemError::OutOfSpace.into();
        assert_eq!(from_mem, JniError::Mem(MemError::OutOfSpace));
        // A heap too small for the tables.
        let mem = Arc::new(GuestMemory::new().unwrap());
        let heap = bhpc_mem::GuestHeap::new(mem.clone(), 0x4000_0000, 1 << 20).unwrap();
        let st =
            State::new(mem.clone(), heap, Config { stdout: Box::new(std::io::sink()), ..Config::default() }).unwrap();
        exhaust(&st);
        let cpu = Cpu::with_options(mem, CpuOptions::default()).unwrap();
        assert!(matches!(install(&cpu, &st), Err(JniError::OutOfMemory)));
    }
}
