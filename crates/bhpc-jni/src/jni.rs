//! The `JNINativeInterface` and `JNIInvokeInterface` function tables as host thunks.
//!
//! Every slot of the 233-entry JNI table is a shared (re-entrant) thunk named `JNI:<Name>`. The
//! `Call*Method*`, `Get/Set*Field`, `New*Array` and `*ArrayRegion` families are one generic body
//! each, parameterised by type.

use crate::invoke::Recv;
use crate::mutf8;
use crate::value::{ClassId, Obj, Ty, Value};
use crate::vm::{Data, Lent, RefKind, Throw, Vm};
use bhpc_cpu::{ArgCursor, Cpu, SharedThunkFn, Stop};
use std::sync::Arc;

/// `JNI_VERSION_1_6`.
pub const JNI_VERSION: u32 = 0x0001_0006;
/// Number of slots of `JNINativeInterface` (including the four reserved ones).
pub const TABLE_SLOTS: usize = 233;

/// Guest addresses of the installed tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The `JavaVM*` handed to `JNI_OnLoad`.
    pub java_vm: u32,
    /// The `JNINativeInterface` function table.
    pub table: u32,
    /// The `JNIInvokeInterface` function table.
    pub invoke_table: u32,
}

const PRIMS: [Ty; 8] = [Ty::Bool, Ty::Byte, Ty::Char, Ty::Short, Ty::Int, Ty::Long, Ty::Float, Ty::Double];
const RETS: [(&str, Ty); 10] = [
    ("Object", Ty::Ref),
    ("Boolean", Ty::Bool),
    ("Byte", Ty::Byte),
    ("Char", Ty::Char),
    ("Short", Ty::Short),
    ("Int", Ty::Int),
    ("Long", Ty::Long),
    ("Float", Ty::Float),
    ("Double", Ty::Double),
    ("Void", Ty::Void),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Var,
    VaList,
    Array,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Virtual,
    Nonvirtual,
    Static,
}

/// Argument and return plumbing of one JNI call.
struct J<'a> {
    vm: &'a Vm,
    cpu: &'a mut Cpu,
    pid: usize,
    a: ArgCursor,
}

impl J<'_> {
    fn u32(&mut self) -> u32 {
        self.a.u32(self.cpu)
    }
    fn u64(&mut self) -> u64 {
        self.a.u64(self.cpu)
    }
    fn obj(&mut self) -> Option<Obj> {
        let h = self.u32();
        self.vm.resolve(h)
    }
    fn class(&mut self) -> Option<ClassId> {
        let o = self.obj()?;
        self.vm.class_of_mirror(o)
    }
    fn cstr(&mut self) -> String {
        let p = self.u32();
        self.vm.mem().read_cstr(p).unwrap_or_default()
    }
    fn ret(&mut self, v: u32) {
        self.cpu.ret_u32(v);
    }
    fn ret_ref(&mut self, o: Option<Obj>) {
        let h = self.vm.new_ref(self.pid, o, RefKind::Local);
        self.cpu.ret_u32(h);
    }
    fn write_u32(&self, ptr: u32, v: u32) {
        if ptr != 0 {
            let _ = self.vm.mem().write_u32(ptr, v);
        }
    }
    /// Report a Java exception to the guest, or stop the CPU for a fatal one.
    fn fail(&mut self, t: Throw) {
        self.cpu.ret_u64(0);
        match t {
            Throw::Exc(e) => self.vm.set_pending(self.pid, e),
            Throw::Stop(s) => self.cpu.request_stop(s),
        }
    }
    fn throw(&mut self, class: &str, msg: &str) {
        self.cpu.ret_u64(0);
        let e = self.vm.new_exception(class, msg);
        self.vm.set_pending(self.pid, e);
    }
    /// Return `v` as the JNI result of type `ty`.
    fn ret_value(&mut self, ty: Ty, v: Value) {
        match ty {
            Ty::Void => {}
            Ty::Ref => self.ret_ref(v.as_ref()),
            Ty::Bool => self.ret((v.as_int() != 0) as u32),
            Ty::Byte => self.ret(v.as_int() as i8 as u32),
            Ty::Char => self.ret(v.as_int() as u16 as u32),
            Ty::Short => self.ret(v.as_int() as i16 as u32),
            Ty::Int => self.ret(v.as_int() as u32),
            Ty::Long => self.cpu.ret_u64(v.as_long() as u64),
            Ty::Float => self.ret(v.as_float().to_bits()),
            Ty::Double => self.cpu.ret_f64(v.as_double()),
        }
    }
    /// Read a value of type `ty` as a plain (non-variadic) argument.
    fn value_arg(&mut self, ty: Ty) -> Value {
        match ty {
            Ty::Long => Value::Long(self.u64() as i64),
            Ty::Double => Value::Double(f64::from_bits(self.u64())),
            Ty::Float => Value::Float(f32::from_bits(self.u32())),
            Ty::Ref => Value::Ref(self.obj()),
            _ => Value::Int(self.u32() as i32),
        }
    }
    /// Pending exception check used by calls that must not run with one pending.
    fn has_pending(&self) -> bool {
        self.vm.pending(self.pid).is_some()
    }
}

fn thunk(vm: &Vm, f: impl Fn(&mut J) + Send + Sync + 'static) -> SharedThunkFn {
    let vm = vm.clone();
    Arc::new(move |cpu: &mut Cpu| {
        let pid = cpu.processor_id();
        let mut a = cpu.args();
        a.u32(cpu); // JNIEnv* / JavaVM*
        let mut j = J { vm: &vm, cpu, pid, a };
        f(&mut j)
    })
}

// ------------------------------------------------------------------ memory helpers

fn read_cbytes(vm: &Vm, mut p: u32) -> Vec<u8> {
    let mut out = Vec::new();
    while out.len() < (1 << 24) {
        match vm.mem().read_u8(p) {
            Ok(0) | Err(_) => break,
            Ok(b) => out.push(b),
        }
        p += 1;
    }
    out
}

fn bounds_ok(start: i32, len: i32, total: usize) -> bool {
    start >= 0 && len >= 0 && (start as usize).checked_add(len as usize).is_some_and(|e| e <= total)
}

// ------------------------------------------------------------------ calls

/// Read the Java arguments of a `Call*` according to the call form.
fn read_args(j: &mut J, params: &[Ty], form: Form) -> Vec<Value> {
    match form {
        Form::Var => params
            .iter()
            .map(|t| match t {
                // Variadic promotion: float travels as double.
                Ty::Float => Value::Float(f64::from_bits(j.u64()) as f32),
                t => j.value_arg(*t),
            })
            .collect(),
        Form::VaList => {
            let mut ap = j.u32();
            let mem = j.vm.mem().clone();
            let mut out = Vec::new();
            for t in params {
                let wide = matches!(t, Ty::Long | Ty::Double | Ty::Float);
                if wide {
                    ap = (ap + 7) & !7;
                }
                let lo = mem.read_u32(ap).unwrap_or(0);
                let hi = if wide { mem.read_u32(ap + 4).unwrap_or(0) } else { 0 };
                ap += if wide { 8 } else { 4 };
                let bits = (hi as u64) << 32 | lo as u64;
                out.push(match t {
                    Ty::Long => Value::Long(bits as i64),
                    Ty::Double => Value::Double(f64::from_bits(bits)),
                    Ty::Float => Value::Float(f64::from_bits(bits) as f32),
                    Ty::Ref => Value::Ref(j.vm.resolve(lo)),
                    _ => Value::Int(lo as i32),
                });
            }
            out
        }
        Form::Array => {
            let base = j.u32();
            let mem = j.vm.mem().clone();
            params
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let at = base + 8 * i as u32;
                    let lo = mem.read_u32(at).unwrap_or(0);
                    match t {
                        Ty::Long => Value::Long(mem.read_u64(at).unwrap_or(0) as i64),
                        Ty::Double => Value::Double(f64::from_bits(mem.read_u64(at).unwrap_or(0))),
                        Ty::Float => Value::Float(f32::from_bits(lo)),
                        Ty::Ref => Value::Ref(j.vm.resolve(lo)),
                        Ty::Bool => Value::Int((lo & 0xff) as i32),
                        Ty::Byte => Value::Int(lo as i8 as i32),
                        Ty::Char => Value::Int((lo & 0xffff) as i32),
                        Ty::Short => Value::Int(lo as i16 as i32),
                        _ => Value::Int(lo as i32),
                    }
                })
                .collect()
        }
    }
}

fn do_call(j: &mut J, kind: Kind, ret: Ty, form: Form) {
    let (recv_obj, class) = match kind {
        Kind::Virtual => (j.obj(), None),
        Kind::Nonvirtual => (j.obj(), Some(j.class())),
        Kind::Static => (None, Some(j.class())),
    };
    let mid = j.u32();
    let Some((mclass, _, sig, _, _)) = j.vm.method_info(mid) else {
        return j.throw("java/lang/NoSuchMethodError", "bad method id");
    };
    let args = read_args(j, &sig.params, form);
    let (recv, virt) = match kind {
        Kind::Static => (Recv::Static(class.flatten().unwrap_or(mclass)), false),
        Kind::Nonvirtual => match recv_obj {
            Some(o) => (Recv::Instance(o), false),
            None => return j.throw("java/lang/NullPointerException", "null receiver"),
        },
        Kind::Virtual => match recv_obj {
            Some(o) => (Recv::Instance(o), true),
            None => return j.throw("java/lang/NullPointerException", "null receiver"),
        },
    };
    let vm = j.vm.clone();
    match vm.invoke(j.cpu, mid, recv, &args, virt) {
        Ok(v) => j.ret_value(ret, v),
        Err(t) => j.fail(t),
    }
}

fn do_new_object(j: &mut J, form: Form) {
    let class = j.class();
    let mid = j.u32();
    let (Some(class), Some((_, _, sig, _, _))) = (class, j.vm.method_info(mid)) else {
        return j.throw("java/lang/NoSuchMethodError", "bad constructor");
    };
    let args = read_args(j, &sig.params, form);
    let vm = j.vm.clone();
    let o = vm.alloc_object(class);
    match vm.invoke(j.cpu, mid, Recv::Instance(o), &args, false) {
        Ok(_) => j.ret_ref(Some(o)),
        Err(t) => j.fail(t),
    }
}

// ------------------------------------------------------------------ fields

fn field_ref(j: &mut J, is_static: bool) -> Option<(Option<Obj>, u32)> {
    let holder = if is_static { None } else { Some(j.obj()?) };
    if is_static {
        j.u32(); // jclass
    }
    let fid = j.u32();
    Some((holder, fid))
}

fn do_get_field(j: &mut J, ty: Ty, is_static: bool) {
    let Some((holder, fid)) = field_ref(j, is_static) else {
        return j.throw("java/lang/NullPointerException", "null object");
    };
    let st = j.vm.st();
    let v = match (st.fields.get(fid as usize).filter(|_| fid != 0), holder) {
        (None, _) => ty.default_value(),
        (Some(f), None) => f.value,
        (Some(f), Some(o)) => st.obj(o).and_then(|x| x.fields.get(&fid).copied()).unwrap_or(f.ty.default_value()),
    };
    drop(st);
    j.ret_value(ty, v);
}

fn do_set_field(j: &mut J, ty: Ty, is_static: bool) {
    let Some((holder, fid)) = field_ref(j, is_static) else {
        return j.throw("java/lang/NullPointerException", "null object");
    };
    let v = j.value_arg(ty);
    let mut st = j.vm.st();
    if fid == 0 || fid as usize >= st.fields.len() {
        return;
    }
    match holder {
        None => st.fields[fid as usize].value = v,
        Some(o) => {
            if let Some(x) = st.obj_mut(o) {
                x.fields.insert(fid, v);
            }
        }
    }
}

// ------------------------------------------------------------------ strings and arrays

/// Lend a freshly allocated guest buffer holding `bytes` for object `obj`.
fn lend(j: &mut J, obj: Obj, bytes: &[u8], extra: u32, is_copy: u32) -> u32 {
    let p = j.vm.galloc_bytes(bytes, extra.max(1));
    if p == 0 {
        j.throw("java/lang/OutOfMemoryError", "guest heap exhausted");
        return 0;
    }
    j.vm.st().lent.insert(p, Lent { obj, len: bytes.len() as u32 });
    // `jboolean *isCopy` is one byte: a word write clobbered the three bytes after it, which for
    // `nativeTouchesMove` was its array-length local (the game then saw zero touches).
    if is_copy != 0 {
        let _ = j.vm.mem().write_u8(is_copy, 1);
    }
    p
}

/// Give a lent buffer back. `copy_back` writes it into the array first; `free` releases it.
fn unlend(j: &mut J, ptr: u32, copy_back: bool, free: bool) {
    let Some((obj, len)) = j.vm.st().lent.get(&ptr).map(|l| (l.obj, l.len)) else { return };
    if copy_back {
        let bytes = j.vm.mem().read_bytes(ptr, len).unwrap_or_default();
        j.vm.with_data(obj, |d| {
            if let Data::Prim(_, v) = d {
                if v.len() == bytes.len() {
                    *v = bytes;
                }
            }
        });
    }
    if free {
        j.vm.st().lent.remove(&ptr);
        j.vm.gfree(ptr);
    }
}

fn prim_bytes(j: &J, arr: Obj) -> Option<(Ty, Vec<u8>)> {
    match j.vm.data(arr)? {
        Data::Prim(t, b) => Some((t, b)),
        _ => None,
    }
}

fn new_prim_array(j: &mut J, ty: Ty) {
    let n = j.u32() as i32;
    if n < 0 {
        return j.throw("java/lang/NegativeArraySizeException", &n.to_string());
    }
    let a = j.vm.new_prim_array(ty, n as usize);
    j.ret_ref(Some(a));
}

fn array_elements(j: &mut J) {
    let (arr, is_copy) = (j.obj(), j.u32());
    let Some((_, bytes)) = arr.and_then(|a| prim_bytes(j, a)) else { return j.ret(0) };
    let p = lend(j, arr.unwrap_or(Obj::NULL), &bytes, 0, is_copy);
    j.ret(p);
}

/// `Release*`: `(env, obj, buffer[, mode])`. Strings have no mode (always free).
fn release_elements(j: &mut J, has_mode: bool) {
    j.u32(); // the array or string
    let ptr = j.u32();
    let mode = if has_mode { j.u32() } else { 0 };
    unlend(j, ptr, mode != 2, mode != 1);
}

fn array_region(j: &mut J, set: bool) {
    let arr = j.obj();
    let (start, len, buf) = (j.u32() as i32, j.u32() as i32, j.u32());
    let Some((ty, bytes)) = arr.and_then(|a| prim_bytes(j, a)) else {
        return j.throw("java/lang/NullPointerException", "array");
    };
    let n = bytes.len() / ty.size();
    if !bounds_ok(start, len, n) {
        return j.throw("java/lang/ArrayIndexOutOfBoundsException", &format!("{start}+{len} of {n}"));
    }
    let (off, cnt) = (start as usize * ty.size(), len as usize * ty.size());
    if set {
        let src = j.vm.mem().read_bytes(buf, cnt as u32).unwrap_or_default();
        j.vm.with_data(arr.unwrap_or(Obj::NULL), |d| {
            if let Data::Prim(_, v) = d {
                v[off..off + src.len()].copy_from_slice(&src);
            }
        });
    } else {
        let _ = j.vm.mem().write_bytes(buf, &bytes[off..off + cnt]);
    }
}

fn string_units(j: &mut J) -> Option<(Obj, Vec<u16>)> {
    let s = j.obj()?;
    Some((s, j.vm.string_units(s)?))
}

fn units_bytes(u: &[u16]) -> Vec<u8> {
    u.iter().flat_map(|c| c.to_le_bytes()).collect()
}

fn do_get_string_chars(j: &mut J) {
    let Some((s, u)) = string_units(j) else { return j.ret(0) };
    let is_copy = j.u32();
    let p = lend(j, s, &units_bytes(&u), 2, is_copy);
    j.ret(p);
}

fn do_get_string_utf_chars(j: &mut J) {
    let Some((s, u)) = string_units(j) else { return j.ret(0) };
    let is_copy = j.u32();
    let p = lend(j, s, &mutf8::encode(&u), 1, is_copy);
    j.ret(p);
}

fn do_string_region(j: &mut J, utf: bool) {
    let Some((_, u)) = string_units(j) else {
        return j.throw("java/lang/NullPointerException", "string");
    };
    let (start, len, buf) = (j.u32() as i32, j.u32() as i32, j.u32());
    if !bounds_ok(start, len, u.len()) {
        return j.throw("java/lang/StringIndexOutOfBoundsException", &format!("{start}+{len} of {}", u.len()));
    }
    let part = &u[start as usize..(start + len) as usize];
    let mut bytes = if utf { mutf8::encode(part) } else { units_bytes(part) };
    if utf {
        bytes.push(0);
    }
    let _ = j.vm.mem().write_bytes(buf, &bytes);
}

// ------------------------------------------------------------------ tables

fn zero(vm: &Vm) -> SharedThunkFn {
    thunk(vm, |j| j.ret(0))
}

/// Build the 233 thunks of `JNINativeInterface`, in slot order, as `(name, thunk)`.
fn native_interface(vm: &Vm) -> Vec<(String, SharedThunkFn)> {
    let mut t: Vec<(String, SharedThunkFn)> = Vec::with_capacity(TABLE_SLOTS);
    macro_rules! add {
        ($name:expr, $f:expr) => {
            t.push(($name.to_string(), thunk(vm, $f)))
        };
    }
    for i in 0..4 {
        t.push((format!("reserved{i}"), zero(vm)));
    }
    add!("GetVersion", |j| j.ret(JNI_VERSION));
    add!("DefineClass", |j| {
        let name = j.cstr();
        j.throw("java/lang/NoClassDefFoundError", &name);
        j.ret(0);
    });
    add!("FindClass", |j| {
        let name = j.cstr();
        match j.vm.find_class(&name) {
            Ok(c) => {
                let m = j.vm.class_object(c);
                j.ret_ref(Some(m));
            }
            Err(e) => {
                j.fail(e);
                j.ret(0);
            }
        }
    });
    t.push(("FromReflectedMethod".into(), zero(vm)));
    t.push(("FromReflectedField".into(), zero(vm)));
    t.push(("ToReflectedMethod".into(), zero(vm)));
    add!("GetSuperclass", |j| {
        let sup = j.class().and_then(|c| j.vm.superclass(c)).map(|s| j.vm.class_object(s));
        j.ret_ref(sup);
    });
    add!("IsAssignableFrom", |j| {
        let (a, b) = (j.class(), j.class());
        let r = matches!((a, b), (Some(a), Some(b)) if j.vm.is_assignable(a, b));
        j.ret(r as u32);
    });
    t.push(("ToReflectedField".into(), zero(vm)));
    add!("Throw", |j| match j.obj() {
        Some(e) => {
            j.vm.set_pending(j.pid, e);
            j.ret(0);
        }
        None => j.ret(u32::MAX),
    });
    add!("ThrowNew", |j| {
        let c = j.class();
        let msg = j.cstr();
        let name = c.map_or("java/lang/NoClassDefFoundError".to_string(), |c| j.vm.class_name(c));
        j.throw(&name, &msg);
        j.ret(0);
    });
    add!("ExceptionOccurred", |j| {
        let e = j.vm.pending(j.pid);
        j.ret_ref(e);
    });
    add!("ExceptionDescribe", |j| {
        if let Some(e) = j.vm.pending(j.pid) {
            let cls = j.vm.class_of(e).map_or(String::new(), |c| j.vm.class_name(c));
            let msg = j.vm.exception_message(e).unwrap_or_default();
            j.vm.log(&format!("jni: exception {cls}: {msg}"));
        }
    });
    add!("ExceptionClear", |j| {
        j.vm.take_pending(j.pid);
    });
    add!("FatalError", |j| {
        let msg = j.cstr();
        j.cpu.request_stop(Stop::Host(format!("JNI FatalError: {msg}")));
    });
    add!("PushLocalFrame", |j| {
        j.vm.push_frame(j.pid);
        j.ret(0);
    });
    add!("PopLocalFrame", |j| {
        let keep = j.obj();
        let h = j.vm.pop_frame(j.pid, keep);
        j.ret(h);
    });
    add!("NewGlobalRef", |j| {
        let o = j.obj();
        let h = j.vm.new_ref(j.pid, o, RefKind::Global);
        j.ret(h);
    });
    add!("DeleteGlobalRef", |j| {
        let h = j.u32();
        j.vm.delete_ref(j.pid, h);
    });
    add!("DeleteLocalRef", |j| {
        let h = j.u32();
        j.vm.delete_ref(j.pid, h);
    });
    add!("IsSameObject", |j| {
        let (a, b) = (j.obj(), j.obj());
        j.ret((a == b) as u32);
    });
    add!("NewLocalRef", |j| {
        let o = j.obj();
        j.ret_ref(o);
    });
    t.push(("EnsureLocalCapacity".into(), zero(vm)));
    add!("AllocObject", |j| match j.class() {
        Some(c) => {
            let o = j.vm.alloc_object(c);
            j.ret_ref(Some(o));
        }
        None => j.ret(0),
    });
    for (name, form) in [("NewObject", Form::Var), ("NewObjectV", Form::VaList), ("NewObjectA", Form::Array)] {
        add!(name, move |j| do_new_object(j, form));
    }
    add!("GetObjectClass", |j| {
        let c = j.obj().and_then(|o| j.vm.class_of(o)).map(|c| j.vm.class_object(c));
        j.ret_ref(c);
    });
    add!("IsInstanceOf", |j| {
        let (o, c) = (j.obj(), j.class());
        let r = match (o, c) {
            (None, _) => true,
            (Some(o), Some(c)) => j.vm.class_of(o).is_some_and(|k| j.vm.is_assignable(k, c)),
            (Some(_), None) => false,
        };
        j.ret(r as u32);
    });
    add!("GetMethodID", |j| method_id(j, false));
    for (prefix, kind) in [("", Kind::Virtual), ("Nonvirtual", Kind::Nonvirtual)] {
        for (rn, ret) in RETS {
            for (suffix, form) in [("", Form::Var), ("V", Form::VaList), ("A", Form::Array)] {
                add!(format!("Call{prefix}{rn}Method{suffix}"), move |j| do_call(j, kind, ret, form));
            }
        }
    }
    add!("GetFieldID", field_id);
    for (rn, ret) in &RETS[..9] {
        let ret = *ret;
        add!(format!("Get{rn}Field"), move |j| do_get_field(j, ret, false));
    }
    for (rn, ret) in &RETS[..9] {
        let ret = *ret;
        add!(format!("Set{rn}Field"), move |j| do_set_field(j, ret, false));
    }
    add!("GetStaticMethodID", |j| method_id(j, true));
    for (rn, ret) in RETS {
        for (suffix, form) in [("", Form::Var), ("V", Form::VaList), ("A", Form::Array)] {
            add!(format!("CallStatic{rn}Method{suffix}"), move |j| do_call(j, Kind::Static, ret, form));
        }
    }
    add!("GetStaticFieldID", field_id);
    for (rn, ret) in &RETS[..9] {
        let ret = *ret;
        add!(format!("GetStatic{rn}Field"), move |j| do_get_field(j, ret, true));
    }
    for (rn, ret) in &RETS[..9] {
        let ret = *ret;
        add!(format!("SetStatic{rn}Field"), move |j| do_set_field(j, ret, true));
    }
    add!("NewString", |j| {
        let (p, n) = (j.u32(), j.u32());
        let raw = j.vm.mem().read_bytes(p, n.saturating_mul(2)).unwrap_or_default();
        let u: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let s = j.vm.new_string_units(u);
        j.ret_ref(Some(s));
    });
    add!("GetStringLength", |j| {
        let n = string_units(j).map_or(0, |(_, u)| u.len());
        j.ret(n as u32);
    });
    add!("GetStringChars", do_get_string_chars);
    add!("ReleaseStringChars", |j| release_elements(j, false));
    add!("NewStringUTF", |j| {
        let p = j.u32();
        if p == 0 {
            return j.ret(0);
        }
        let s = j.vm.new_string_units(mutf8::decode(&read_cbytes(j.vm, p)));
        j.ret_ref(Some(s));
    });
    add!("GetStringUTFLength", |j| {
        let n = string_units(j).map_or(0, |(_, u)| mutf8::encode(&u).len());
        j.ret(n as u32);
    });
    add!("GetStringUTFChars", do_get_string_utf_chars);
    add!("ReleaseStringUTFChars", |j| release_elements(j, false));
    add!("GetArrayLength", |j| {
        let n = j.obj().and_then(|a| j.vm.array_len(a)).unwrap_or(0);
        j.ret(n as u32);
    });
    add!("NewObjectArray", |j| {
        let n = j.u32() as i32;
        let (c, init) = (j.class(), j.obj());
        if n < 0 {
            return j.throw("java/lang/NegativeArraySizeException", &n.to_string());
        }
        let elem = c.map_or("java/lang/Object".to_string(), |c| j.vm.class_name(c));
        let a = j.vm.new_object_array(&elem, n as usize);
        if init.is_some() {
            j.vm.with_data(a, |d| *d = Data::Objs(vec![init; n as usize]));
        }
        j.ret_ref(Some(a));
    });
    add!("GetObjectArrayElement", |j| {
        let (a, i) = (j.obj(), j.u32() as i32);
        match a.and_then(|a| j.vm.data(a)) {
            Some(Data::Objs(v)) if bounds_ok(i, 1, v.len()) => j.ret_ref(v[i as usize]),
            _ => {
                j.throw("java/lang/ArrayIndexOutOfBoundsException", &i.to_string());
                j.ret(0);
            }
        }
    });
    add!("SetObjectArrayElement", |j| {
        let (a, i, v) = (j.obj(), j.u32() as i32, j.obj());
        let ok = a.and_then(|a| {
            j.vm.with_data(a, |d| match d {
                Data::Objs(items) if bounds_ok(i, 1, items.len()) => {
                    items[i as usize] = v;
                    true
                }
                _ => false,
            })
        });
        if ok != Some(true) {
            j.throw("java/lang/ArrayIndexOutOfBoundsException", &i.to_string());
        }
    });
    for ty in PRIMS {
        add!(format!("New{}Array", RETS.iter().find(|r| r.1 == ty).map_or("", |r| r.0)), move |j| new_prim_array(
            j, ty
        ));
    }
    for (rn, _) in RETS.iter().filter(|r| PRIMS.contains(&r.1)) {
        add!(format!("Get{rn}ArrayElements"), array_elements);
    }
    for (rn, _) in RETS.iter().filter(|r| PRIMS.contains(&r.1)) {
        add!(format!("Release{rn}ArrayElements"), |j| release_elements(j, true));
    }
    for (rn, _) in RETS.iter().filter(|r| PRIMS.contains(&r.1)) {
        add!(format!("Get{rn}ArrayRegion"), |j| array_region(j, false));
    }
    for (rn, _) in RETS.iter().filter(|r| PRIMS.contains(&r.1)) {
        add!(format!("Set{rn}ArrayRegion"), |j| array_region(j, true));
    }
    add!("RegisterNatives", |j| {
        let (c, methods, n) = (j.class(), j.u32(), j.u32());
        let Some(c) = c else { return j.ret(u32::MAX) };
        for i in 0..n {
            let at = methods + 12 * i;
            let (np, sp, fp) = {
                let m = j.vm.mem();
                (m.read_u32(at).unwrap_or(0), m.read_u32(at + 4).unwrap_or(0), m.read_u32(at + 8).unwrap_or(0))
            };
            let (name, sig) = (
                String::from_utf8_lossy(&read_cbytes(j.vm, np)).into_owned(),
                String::from_utf8_lossy(&read_cbytes(j.vm, sp)).into_owned(),
            );
            if !j.vm.register_native(c, &name, &sig, fp) {
                j.throw("java/lang/NoSuchMethodError", &format!("{name}{sig}"));
                return j.ret(u32::MAX);
            }
        }
        j.ret(0);
    });
    add!("UnregisterNatives", |j| j.ret(0));
    t.push(("MonitorEnter".into(), zero(vm)));
    t.push(("MonitorExit".into(), zero(vm)));
    add!("GetJavaVM", |j| {
        let out = j.u32();
        let vmp = j.vm.0.layout.get().map_or(0, |l| l.java_vm);
        j.write_u32(out, vmp);
        j.ret(0);
    });
    add!("GetStringRegion", |j| do_string_region(j, false));
    add!("GetStringUTFRegion", |j| do_string_region(j, true));
    add!("GetPrimitiveArrayCritical", array_elements);
    add!("ReleasePrimitiveArrayCritical", |j| release_elements(j, true));
    add!("GetStringCritical", do_get_string_chars);
    add!("ReleaseStringCritical", |j| release_elements(j, false));
    add!("NewWeakGlobalRef", |j| {
        let o = j.obj();
        let h = j.vm.new_ref(j.pid, o, RefKind::Weak);
        j.ret(h);
    });
    add!("DeleteWeakGlobalRef", |j| {
        let h = j.u32();
        j.vm.delete_ref(j.pid, h);
    });
    add!("ExceptionCheck", |j| {
        let p = j.has_pending();
        j.ret(p as u32);
    });
    add!("NewDirectByteBuffer", |j| {
        let (addr, cap) = (j.u32(), j.u64());
        let b = j.vm.new_data_object("java/nio/DirectByteBuffer", Data::None);
        j.vm.set_field(b, "address", Value::Long(addr as i64));
        j.vm.set_field(b, "capacity", Value::Long(cap as i64));
        j.ret_ref(Some(b));
    });
    add!("GetDirectBufferAddress", |j| {
        let a = j.obj().map_or(0, |b| j.vm.get_field(b, "address").as_long());
        j.ret(a as u32);
    });
    add!("GetDirectBufferCapacity", |j| {
        let c = j.obj().map_or(-1, |b| j.vm.get_field(b, "capacity").as_long());
        j.cpu.ret_u64(c as u64);
    });
    add!("GetObjectRefType", |j| {
        let h = j.u32();
        let t = j.vm.ref_type(h);
        j.ret(t);
    });
    t
}

fn method_id(j: &mut J, is_static: bool) {
    let c = j.class();
    let (name, sig) = (j.cstr(), j.cstr());
    let Some(c) = c else {
        j.throw("java/lang/NoClassDefFoundError", &name);
        return j.ret(0);
    };
    match j.vm.method_id(c, &name, &sig, is_static) {
        Ok(m) => j.ret(m),
        Err(e) => {
            j.fail(e);
            j.ret(0);
        }
    }
}

fn field_id(j: &mut J) {
    let c = j.class();
    let (name, sig) = (j.cstr(), j.cstr());
    let Some(c) = c else {
        j.throw("java/lang/NoClassDefFoundError", &name);
        return j.ret(0);
    };
    match j.vm.jfield_id(c, &name, &sig) {
        Ok(f) => j.ret(f),
        Err(e) => {
            j.fail(e);
            j.ret(0);
        }
    }
}

/// `JNI_VERSION_1_1` .. `JNI_VERSION_1_6`.
fn version_ok(v: u32) -> bool {
    (0x0001_0001..=JNI_VERSION).contains(&v)
}

fn invoke_interface(vm: &Vm) -> Vec<(String, SharedThunkFn)> {
    let mut t: Vec<(String, SharedThunkFn)> = Vec::new();
    for i in 0..3 {
        t.push((format!("vm_reserved{i}"), zero(vm)));
    }
    t.push(("DestroyJavaVM".into(), zero(vm)));
    let attach = |vm: &Vm| {
        thunk(vm, |j| {
            let out = j.u32();
            let env = j.vm.env_ptr(j.pid);
            j.write_u32(out, env);
            j.ret(0);
        })
    };
    t.push(("AttachCurrentThread".into(), attach(vm)));
    t.push(("DetachCurrentThread".into(), zero(vm)));
    t.push((
        "GetEnv".into(),
        thunk(vm, |j| {
            let (out, version) = (j.u32(), j.u32());
            if version_ok(version) {
                let env = j.vm.env_ptr(j.pid);
                j.write_u32(out, env);
                j.ret(0);
            } else {
                j.write_u32(out, 0);
                j.ret(-3i32 as u32);
            }
        }),
    ));
    t.push(("AttachCurrentThreadAsDaemon".into(), attach(vm)));
    t
}

impl Vm {
    /// Register all JNI thunks on `cpu`'s thunk table and build the guest-visible tables. Call it
    /// once per guest memory; the table is shared with every thread CPU.
    pub fn install(&self, cpu: &Cpu) -> Result<Layout, String> {
        if let Some(l) = self.0.layout.get() {
            return Ok(*l);
        }
        let mem = self.mem().clone();
        let table = self.galloc(4 * TABLE_SLOTS as u32);
        let invoke_table = self.galloc(4 * 8);
        let java_vm = self.galloc(4);
        if table == 0 || invoke_table == 0 || java_vm == 0 {
            return Err("guest heap exhausted while building the JNI tables".into());
        }
        let werr = |e: bhpc_mem::MemError| e.to_string();
        for (base, items) in [(table, native_interface(self)), (invoke_table, invoke_interface(self))] {
            for (i, (name, f)) in items.into_iter().enumerate() {
                let addr = cpu.register_shared_thunk(&format!("JNI:{name}"), f).map_err(|e| format!("{e:?}"))?;
                mem.write_u32(base + 4 * i as u32, addr).map_err(werr)?;
            }
        }
        mem.write_u32(java_vm, invoke_table).map_err(werr)?;
        let l = Layout { java_vm, table, invoke_table };
        Ok(*self.0.layout.get_or_init(|| l))
    }

    /// The installed table addresses (None before [`Vm::install`]).
    pub fn layout(&self) -> Option<Layout> {
        self.0.layout.get().copied()
    }

    /// The `JNIEnv*` of guest thread `pid` (one per [`Cpu::processor_id`]); 0 before install.
    pub fn env_ptr(&self, pid: usize) -> u32 {
        let Some(l) = self.0.layout.get() else { return 0 };
        if let Some(e) = self.st().envs.get(&pid) {
            return *e;
        }
        let e = self.galloc(4);
        if e == 0 || self.mem().write_u32(e, l.table).is_err() {
            return 0;
        }
        self.st().envs.insert(pid, e);
        e
    }
}
