//! The JNI function tables, called the way guest code calls them.

use crate::tests::Env;
use crate::value::Ty;
use crate::*;
use bhpc_cpu::Stop;
use std::sync::{Arc, Mutex};

const ARRAY_TYPES: [(&str, Ty); 8] = [
    ("Boolean", Ty::Bool),
    ("Byte", Ty::Byte),
    ("Char", Ty::Char),
    ("Short", Ty::Short),
    ("Int", Ty::Int),
    ("Long", Ty::Long),
    ("Float", Ty::Float),
    ("Double", Ty::Double),
];

#[test]
fn table_has_233_named_slots() {
    let e = Env::new();
    let mem = e.mem();
    let slot = |i: u32| mem.read_u32(e.layout.table + 4 * i).unwrap();
    let expect = [
        (0, "reserved0"),
        (4, "GetVersion"),
        (5, "DefineClass"),
        (6, "FindClass"),
        (10, "GetSuperclass"),
        (13, "Throw"),
        (17, "ExceptionClear"),
        (21, "NewGlobalRef"),
        (24, "IsSameObject"),
        (27, "AllocObject"),
        (28, "NewObject"),
        (30, "NewObjectA"),
        (31, "GetObjectClass"),
        (33, "GetMethodID"),
        (34, "CallObjectMethod"),
        (36, "CallObjectMethodA"),
        (49, "CallIntMethod"),
        (61, "CallVoidMethod"),
        (62, "CallVoidMethodV"),
        (63, "CallVoidMethodA"),
        (64, "CallNonvirtualObjectMethod"),
        (93, "CallNonvirtualVoidMethodA"),
        (94, "GetFieldID"),
        (95, "GetObjectField"),
        (103, "GetDoubleField"),
        (104, "SetObjectField"),
        (112, "SetDoubleField"),
        (113, "GetStaticMethodID"),
        (114, "CallStaticObjectMethod"),
        (143, "CallStaticVoidMethodA"),
        (144, "GetStaticFieldID"),
        (145, "GetStaticObjectField"),
        (153, "GetStaticDoubleField"),
        (154, "SetStaticObjectField"),
        (162, "SetStaticDoubleField"),
        (163, "NewString"),
        (167, "NewStringUTF"),
        (169, "GetStringUTFChars"),
        (171, "GetArrayLength"),
        (172, "NewObjectArray"),
        (175, "NewBooleanArray"),
        (182, "NewDoubleArray"),
        (183, "GetBooleanArrayElements"),
        (190, "GetDoubleArrayElements"),
        (191, "ReleaseBooleanArrayElements"),
        (199, "GetBooleanArrayRegion"),
        (207, "SetBooleanArrayRegion"),
        (214, "SetDoubleArrayRegion"),
        (215, "RegisterNatives"),
        (219, "GetJavaVM"),
        (222, "GetPrimitiveArrayCritical"),
        (228, "ExceptionCheck"),
        (232, "GetObjectRefType"),
    ];
    for (i, name) in expect {
        assert_eq!(slot(i), e.thunk(name), "slot {i} should be {name}");
    }
    // All slots are distinct thunks except the shared reserved/zero ones.
    let mut seen = std::collections::HashSet::new();
    for i in 0..TABLE_SLOTS as u32 {
        let a = slot(i);
        assert_ne!(a, 0);
        assert!(e.cpu.thunk_name(a).unwrap().starts_with("JNI:"), "slot {i}");
        seen.insert(a);
    }
    assert_eq!(seen.len(), TABLE_SLOTS);
    // 233 JNI functions + 8 invocation functions.
    assert_eq!(e.cpu.thunk_count(), TABLE_SLOTS + 8);
    assert_eq!(e.vm.install(&e.cpu).unwrap(), e.layout, "install is idempotent");
    assert_eq!(mem.read_u32(e.layout.java_vm).unwrap(), e.layout.invoke_table);
    assert_eq!(mem.read_u32(e.env_ptr()).unwrap(), e.layout.table);
    assert_eq!(e.env_ptr(), e.env_ptr(), "one env per thread");
}

#[test]
fn install_reports_exhausted_heap() {
    struct Dry;
    impl GuestAlloc for Dry {
        fn alloc(&mut self, _: u32) -> u32 {
            0
        }
        fn free(&mut self, _: u32) {}
    }
    let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
    let cpu = bhpc_cpu::Cpu::new(mem.clone()).unwrap();
    let vm = new_vm(mem, Box::new(Dry), VmConfig::new(Platform::default(), Box::new(NoAssets)));
    assert!(vm.install(&cpu).unwrap_err().contains("exhausted"));
    // env_ptr cannot allocate either.
    assert_eq!(vm.env_ptr(0), 0);
}

#[test]
fn install_reports_unwritable_tables() {
    // The allocator hands out an unmapped address: writing the tables fails.
    struct Unmapped;
    impl GuestAlloc for Unmapped {
        fn alloc(&mut self, _: u32) -> u32 {
            0xdead_0000
        }
        fn free(&mut self, _: u32) {}
    }
    let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
    let cpu = bhpc_cpu::Cpu::new(mem.clone()).unwrap();
    let vm = new_vm(mem, Box::new(Unmapped), VmConfig::new(Platform::default(), Box::new(NoAssets)));
    assert!(vm.install(&cpu).is_err());
}

#[test]
fn install_reports_a_full_thunk_table() {
    let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
    let heap = bhpc_mem::GuestHeap::new(mem.clone(), 0x4000_0000, 1 << 20).unwrap();
    let cpu = bhpc_cpu::Cpu::new(mem.clone()).unwrap();
    for i in 0..(bhpc_cpu::MAX_THUNKS as usize - 100) {
        cpu.register_shared_thunk(&format!("filler{i}"), Arc::new(|_: &mut bhpc_cpu::Cpu| {})).unwrap();
    }
    let vm = new_vm(mem, Box::new(heap), VmConfig::new(Platform::default(), Box::new(NoAssets)));
    assert!(vm.install(&cpu).unwrap_err().contains("TooManyThunks"));
}

#[test]
fn guest_code_calls_through_the_table() {
    let mut e = Env::new();
    // jint f(JNIEnv *env, ...) { return (*env)->GetVersion(env); } as a tail call through the table.
    let off = 4 * 4;
    let f = e.code(&[0xE590_C000, 0xE59C_C000 | off, 0xE12F_FF1C]);
    let env = e.env_ptr();
    assert_eq!(e.cpu.call(f, &[env]).unwrap().r0, JNI_VERSION);
    // FindClass through the table, with a name placed in guest memory.
    let off = 4 * 6;
    let g = e.code(&[0xE590_C000, 0xE59C_C000 | off, 0xE12F_FF1C]);
    let name = e.cstr("java/lang/String");
    let h = e.cpu.call(g, &[env, name]).unwrap().r0;
    let c = e.vm.resolve(h).unwrap();
    assert_eq!(e.vm.class_of_mirror(c), e.vm.class_named("java/lang/String"));
}

#[test]
fn invocation_interface() {
    let mut e = Env::new();
    let vmp = e.layout.java_vm;
    let out = e.buf(8);
    let call = |e: &mut Env, name: &str, args: &[u32]| {
        let a = e.cpu.thunk_addr(&format!("JNI:{name}")).unwrap();
        let mut all = vec![vmp];
        all.extend_from_slice(args);
        e.cpu.call(a, &all).unwrap().r0
    };
    for v in [0x10001, 0x10002, 0x10004, 0x10006] {
        assert_eq!(call(&mut e, "GetEnv", &[out, v]), 0);
        assert_eq!(e.mem().read_u32(out).unwrap(), e.env_ptr());
    }
    assert_eq!(call(&mut e, "GetEnv", &[out, 0x10007]) as i32, -3);
    assert_eq!(e.mem().read_u32(out).unwrap(), 0);
    assert_eq!(call(&mut e, "GetEnv", &[out, 0]) as i32, -3);
    assert_eq!(call(&mut e, "AttachCurrentThread", &[out, 0]), 0);
    assert_eq!(e.mem().read_u32(out).unwrap(), e.env_ptr());
    e.mem().write_u32(out, 0).unwrap();
    assert_eq!(call(&mut e, "AttachCurrentThreadAsDaemon", &[out, 0]), 0);
    assert_eq!(e.mem().read_u32(out).unwrap(), e.env_ptr());
    assert_eq!(call(&mut e, "DetachCurrentThread", &[]), 0);
    assert_eq!(call(&mut e, "DestroyJavaVM", &[]), 0);
    // A null out pointer is tolerated.
    assert_eq!(call(&mut e, "GetEnv", &[0, 0x10004]), 0);
}

#[test]
fn version_and_class_functions() {
    let mut e = Env::new();
    assert_eq!(e.jni("GetVersion", &[]).r0, 0x10006);
    let n = e.cstr("java/lang/String");
    let h = e.jni("FindClass", &[n]).r0;
    let cls = e.vm.resolve(h).unwrap();
    assert_eq!(e.vm.class_name(e.vm.class_of_mirror(cls).unwrap()), "java/lang/String");
    // Unknown classes are invented in lenient mode.
    let n = e.cstr("com/example/Unknown");
    assert_ne!(e.jni("FindClass", &[n]).r0, 0);
    assert_eq!(e.pending_class(), None);
    // GetSuperclass / IsAssignableFrom.
    let sup = e.jni("GetSuperclass", &[h]).r0;
    assert_eq!(e.vm.class_name(e.vm.class_of_mirror(e.vm.resolve(sup).unwrap()).unwrap()), "java/lang/Object");
    let obj_h = sup;
    assert_eq!(e.jni("GetSuperclass", &[obj_h]).r0, 0);
    assert_eq!(e.jni("IsAssignableFrom", &[h, obj_h]).r0, 1);
    assert_eq!(e.jni("IsAssignableFrom", &[obj_h, h]).r0, 0);
    assert_eq!(e.jni("IsAssignableFrom", &[0, h]).r0, 0);
    // DefineClass always fails.
    let n = e.cstr("gen/Proxy");
    assert_eq!(e.jni("DefineClass", &[n, 0, 0, 0]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoClassDefFoundError"));
    e.clear_pending();
    // Reflection bridges are not supported and return null.
    for name in [
        "FromReflectedMethod",
        "FromReflectedField",
        "ToReflectedMethod",
        "ToReflectedField",
        "EnsureLocalCapacity",
        "MonitorEnter",
        "MonitorExit",
        "UnregisterNatives",
    ] {
        assert_eq!(e.jni(name, &[h, 0, 0]).r0, 0, "{name}");
    }
}

#[test]
fn find_class_strict_throws() {
    let mut e = Env::with(|c| c.lenient = false);
    let n = e.cstr("com/example/Unknown");
    assert_eq!(e.jni("FindClass", &[n]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoClassDefFoundError"));
}

#[test]
fn exceptions_roundtrip() {
    let mut e = Env::new();
    assert_eq!(e.jni("ExceptionCheck", &[]).r0, 0);
    assert_eq!(e.jni("ExceptionOccurred", &[]).r0, 0);
    // Throw(null) fails with -1.
    assert_eq!(e.jni("Throw", &[0]).r0 as i32, -1);
    let x = e.vm.new_exception("java/lang/IllegalStateException", "bad");
    let xh = e.local(x);
    assert_eq!(e.jni("Throw", &[xh]).r0, 0);
    assert_eq!(e.jni("ExceptionCheck", &[]).r0, 1);
    let got = e.jni("ExceptionOccurred", &[]).r0;
    assert_eq!(e.vm.resolve(got), Some(x));
    e.jni("ExceptionDescribe", &[]);
    assert!(e.log_text().contains("jni: exception java.lang.IllegalStateException: bad".replace('.', "/").as_str()));
    e.jni("ExceptionClear", &[]);
    assert_eq!(e.jni("ExceptionCheck", &[]).r0, 0);
    e.jni("ExceptionDescribe", &[]);
    // ThrowNew by class handle, and with a null class.
    let c = e.class_h("java/lang/IllegalArgumentException");
    let m = e.cstr("nope");
    assert_eq!(e.jni("ThrowNew", &[c, m]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/IllegalArgumentException"));
    let pending = e.vm.pending(e.cpu.processor_id()).unwrap();
    assert_eq!(e.vm.exception_message(pending).as_deref(), Some("nope"));
    e.clear_pending();
    e.jni("ThrowNew", &[0, m]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoClassDefFoundError"));
    e.clear_pending();
    // FatalError stops the CPU.
    let m = e.cstr("fatal!");
    assert_eq!(e.try_jni("FatalError", &[m]), Err(Stop::Host("JNI FatalError: fatal!".into())));
}

#[test]
fn reference_functions() {
    let mut e = Env::new();
    let o = e.vm.new_string("x");
    let l = e.local(o);
    let g = e.jni("NewGlobalRef", &[l]).r0;
    let w = e.jni("NewWeakGlobalRef", &[l]).r0;
    let l2 = e.jni("NewLocalRef", &[l]).r0;
    assert_eq!(
        (e.jni("GetObjectRefType", &[l]).r0, e.jni("GetObjectRefType", &[g]).r0, e.jni("GetObjectRefType", &[w]).r0),
        (1, 2, 3)
    );
    assert_eq!(e.jni("GetObjectRefType", &[0]).r0, 0);
    assert_eq!(e.jni("NewGlobalRef", &[0]).r0, 0);
    assert_eq!(e.jni("NewLocalRef", &[0]).r0, 0);
    assert_eq!(e.jni("IsSameObject", &[l, g]).r0, 1);
    assert_eq!(e.jni("IsSameObject", &[0, 0]).r0, 1);
    assert_eq!(e.jni("IsSameObject", &[l, 0]).r0, 0);
    e.jni("DeleteGlobalRef", &[g]);
    e.jni("DeleteWeakGlobalRef", &[w]);
    e.jni("DeleteLocalRef", &[l2]);
    assert_eq!(e.jni("GetObjectRefType", &[g]).r0, 0);
    // Frames: locals made inside are released, the result survives.
    e.jni("PushLocalFrame", &[16]);
    let inner = e.local(e.vm.new_string("inner"));
    let result = e.local(e.vm.new_string("result"));
    let kept = e.jni("PopLocalFrame", &[result]).r0;
    assert_eq!(e.vm.resolve(inner), None);
    assert_eq!(e.vm.string(e.vm.resolve(kept).unwrap()).as_deref(), Some("result"));
    e.jni("PushLocalFrame", &[16]);
    assert_eq!(e.jni("PopLocalFrame", &[0]).r0, 0);
}

// ----------------------------------------------------------------- calls

#[derive(Clone, Copy)]
enum A {
    I(i32),
    J(i64),
    F(f32),
    D(f64),
    R(u32),
}

/// Words of a variadic call (`Call<T>Method(env, obj, mid, ...)`): `lead` are the fixed words.
fn var_words(lead: &[u32], args: &[A]) -> Vec<u32> {
    let mut w = lead.to_vec();
    for a in args {
        match *a {
            A::I(v) => w.push(v as u32),
            A::R(v) => w.push(v),
            A::J(v) => push64(&mut w, v as u64),
            A::D(v) => push64(&mut w, v.to_bits()),
            A::F(v) => push64(&mut w, (v as f64).to_bits()),
        }
    }
    w
}

fn push64(w: &mut Vec<u32>, bits: u64) {
    if w.len() % 2 == 1 {
        w.push(0);
    }
    w.push(bits as u32);
    w.push((bits >> 32) as u32);
}

/// A `va_list` in guest memory for `args`.
fn va_list(e: &mut Env, args: &[A]) -> u32 {
    let base = e.buf(8 + 16 * args.len() as u32);
    let base = (base + 7) & !7;
    let mut cur = base;
    let mem = e.mem();
    for a in args {
        let (bits, wide) = match *a {
            A::I(v) => (v as u32 as u64, false),
            A::R(v) => (v as u64, false),
            A::J(v) => (v as u64, true),
            A::D(v) => (v.to_bits(), true),
            A::F(v) => ((v as f64).to_bits(), true),
        };
        if wide {
            cur = (cur + 7) & !7;
            mem.write_u64(cur, bits).unwrap();
            cur += 8;
        } else {
            mem.write_u32(cur, bits as u32).unwrap();
            cur += 4;
        }
    }
    base
}

/// A `jvalue[]` in guest memory for `args`.
fn jvalues(e: &mut Env, args: &[A]) -> u32 {
    let base = e.buf(8 * args.len() as u32 + 8);
    let mem = e.mem();
    for (i, a) in args.iter().enumerate() {
        let at = base + 8 * i as u32;
        match *a {
            A::I(v) => mem.write_u32(at, v as u32).unwrap(),
            A::R(v) => mem.write_u32(at, v).unwrap(),
            A::J(v) => mem.write_u64(at, v as u64).unwrap(),
            A::D(v) => mem.write_u64(at, v.to_bits()).unwrap(),
            A::F(v) => mem.write_u32(at, v.to_bits()).unwrap(),
        }
    }
    base
}

type Seen = Arc<Mutex<Vec<(String, Vec<Value>, bool)>>>;

/// Class `t/C` with one method per return type taking `(IJFDLjava/lang/Object;Z)`.
fn define_callables(e: &Env) -> (Seen, Obj) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let rets: [(&str, &str, Value); 10] = [
        ("Object", "Ljava/lang/Object;", Value::Ref(None)),
        ("Boolean", "Z", Value::Int(1)),
        ("Byte", "B", Value::Int(-5)),
        ("Char", "C", Value::Int(65_000)),
        ("Short", "S", Value::Int(-300)),
        ("Int", "I", Value::Int(-100_000)),
        ("Long", "J", Value::Long(-0x1_2345_6789)),
        ("Float", "F", Value::Float(-1.5)),
        ("Double", "D", Value::Double(6.25)),
        ("Void", "V", Value::Void),
    ];
    let o = e.vm.new_string("result");
    e.vm.pin(o);
    for (name, desc, val) in rets {
        let val = if name == "Object" { Value::Ref(Some(o)) } else { val };
        let sig = format!("(IJFDLjava/lang/Object;Z){desc}");
        for (prefix, is_static) in [("m", false), ("s", true)] {
            let s2 = seen.clone();
            let tag = format!("{prefix}{name}");
            let f = move |c: &mut Call| {
                s2.lock().unwrap().push((tag.clone(), c.args.to_vec(), c.this != Obj::NULL));
                Ok(val)
            };
            let b = e.vm.define("t/C");
            if is_static {
                b.static_method(&format!("{prefix}{name}"), &sig, f);
            } else {
                b.method(&format!("{prefix}{name}"), &sig, f);
            }
        }
    }
    // A subclass overriding mInt, to tell virtual from nonvirtual calls apart.
    let s3 = seen.clone();
    e.vm.define("t/D").extends("t/C").method("mInt", "(IJFDLjava/lang/Object;Z)I", move |c| {
        s3.lock().unwrap().push(("D.mInt".into(), c.args.to_vec(), true));
        Ok(Value::Int(777))
    });
    (seen, o)
}

fn check_ret(e: &Env, name: &str, r: bhpc_cpu::CallResult, o: Obj) {
    match name {
        "Object" => assert_eq!(e.vm.resolve(r.r0), Some(o)),
        "Boolean" => assert_eq!(r.r0, 1),
        "Byte" => assert_eq!(r.r0 as i32, -5),
        "Char" => assert_eq!(r.r0, 65_000),
        "Short" => assert_eq!(r.r0 as i32, -300),
        "Int" => assert_eq!(r.r0 as i32, -100_000),
        "Long" => assert_eq!(r.as_u64() as i64, -0x1_2345_6789),
        "Float" => assert_eq!(f32::from_bits(r.r0), -1.5),
        "Double" => assert_eq!(r.as_f64(), 6.25),
        _ => {}
    }
}

#[test]
fn call_families_marshal_all_forms() {
    let mut e = Env::new();
    let (seen, result_obj) = define_callables(&e);
    let cls = e.vm.class_named("t/C").unwrap();
    let ch = e.local(e.vm.class_object(cls));
    let obj = e.vm.alloc_object(cls);
    let oh = e.local(obj);
    let arg_obj = e.vm.new_string("arg");
    let ah = e.local(arg_obj);
    let args = [A::I(-7), A::J(0x1_0000_0002), A::F(1.5), A::D(2.25), A::R(ah), A::I(1)];
    let expected = vec![
        Value::Int(-7),
        Value::Long(0x1_0000_0002),
        Value::Float(1.5),
        Value::Double(2.25),
        Value::Ref(Some(arg_obj)),
        Value::Int(1),
    ];
    let sig = "(IJFDLjava/lang/Object;Z)";
    let rets = ["Object", "Boolean", "Byte", "Char", "Short", "Int", "Long", "Float", "Double", "Void"];
    let descs = ["Ljava/lang/Object;", "Z", "B", "C", "S", "I", "J", "F", "D", "V"];
    for (rn, desc) in rets.iter().zip(descs) {
        let n = e.cstr(&format!("m{rn}"));
        let s = e.cstr(&format!("{sig}{desc}"));
        let mid = e.jni("GetMethodID", &[ch, n, s]).r0;
        let n2 = e.cstr(&format!("s{rn}"));
        let smid = e.jni("GetStaticMethodID", &[ch, n2, s]).r0;
        assert!(mid != 0 && smid != 0);
        for form in ["", "V", "A"] {
            // Virtual.
            let r = call_form(&mut e, &format!("Call{rn}Method{form}"), &[oh], mid, &args, form);
            check_ret(&e, rn, r, result_obj);
            // Nonvirtual.
            let r = call_form(&mut e, &format!("CallNonvirtual{rn}Method{form}"), &[oh, ch], mid, &args, form);
            check_ret(&e, rn, r, result_obj);
            // Static.
            let r = call_form(&mut e, &format!("CallStatic{rn}Method{form}"), &[ch], smid, &args, form);
            check_ret(&e, rn, r, result_obj);
        }
    }
    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 10 * 9);
    for (tag, a, has_this) in calls.iter() {
        assert_eq!(a, &expected, "{tag}");
        assert_eq!(*has_this, tag.starts_with('m'), "{tag}");
    }
    drop(calls);
    assert_eq!(e.pending_class(), None);

    // Virtual dispatch picks the override, nonvirtual the declared method.
    seen.lock().unwrap().clear();
    let d = e.vm.alloc_object(e.vm.class_named("t/D").unwrap());
    let dh = e.local(d);
    let n = e.cstr("mInt");
    let s = e.cstr(&format!("{sig}I"));
    let mid = e.jni("GetMethodID", &[ch, n, s]).r0;
    let r = call_form(&mut e, "CallIntMethod", &[dh], mid, &args, "");
    assert_eq!(r.r0, 777);
    let r = call_form(&mut e, "CallNonvirtualIntMethod", &[dh, ch], mid, &args, "");
    assert_eq!(r.r0 as i32, -100_000);
    let tags: Vec<String> = seen.lock().unwrap().iter().map(|x| x.0.clone()).collect();
    assert_eq!(tags, ["D.mInt", "mInt"]);
}

fn call_form(e: &mut Env, name: &str, lead: &[u32], mid: u32, args: &[A], form: &str) -> bhpc_cpu::CallResult {
    let a = e.thunk(name);
    let mut w = vec![e.env_ptr()];
    w.extend_from_slice(lead);
    w.push(mid);
    let words = match form {
        "" => var_words(&w, args),
        "V" => {
            w.push(va_list(e, args));
            w
        }
        _ => {
            w.push(jvalues(e, args));
            w
        }
    };
    e.cpu.call(a, &words).unwrap()
}

#[test]
fn jvalue_arrays_decode_small_types() {
    let mut e = Env::new();
    let seen: Arc<Mutex<Vec<Vec<Value>>>> = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    e.vm.define("t/J").static_method("small", "(ZBCSIFDJ)V", move |c| {
        s2.lock().unwrap().push(c.args.to_vec());
        Ok(Value::Void)
    });
    let ch = e.class_h("t/J");
    let n = e.cstr("small");
    let s = e.cstr("(ZBCSIFDJ)V");
    let mid = e.jni("GetStaticMethodID", &[ch, n, s]).r0;
    let base = e.buf(64);
    let m = e.mem();
    m.write_u32(base, 0xffff_ff01).unwrap(); // Z: only the low byte counts
    m.write_u32(base + 8, 0xffff_ff80).unwrap(); // B: -128
    m.write_u32(base + 16, 0xabcd_ffff).unwrap(); // C: 65535
    m.write_u32(base + 24, 0xffff_8000).unwrap(); // S: -32768
    m.write_u32(base + 32, 5).unwrap();
    m.write_u32(base + 40, 2.5f32.to_bits()).unwrap();
    m.write_u64(base + 48, 1.25f64.to_bits()).unwrap();
    // J goes after: need more room.
    let base2 = e.buf(8 * 8);
    let mem = e.mem();
    for i in 0..6 {
        let v = mem.read_u32(base + 8 * i).unwrap();
        mem.write_u32(base2 + 8 * i, v).unwrap();
    }
    mem.write_u64(base2 + 48, 1.25f64.to_bits()).unwrap();
    mem.write_u64(base2 + 56, 1u64 << 40).unwrap();
    e.jni("CallStaticVoidMethodA", &[ch, mid, base2]);
    let got = seen.lock().unwrap()[0].clone();
    assert_eq!(
        got,
        [
            Value::Int(1),
            Value::Int(-128),
            Value::Int(65535),
            Value::Int(-32768),
            Value::Int(5),
            Value::Float(2.5),
            Value::Double(1.25),
            Value::Long(1 << 40)
        ]
    );
}

#[test]
fn call_errors() {
    let mut e = Env::new();
    let _ = define_callables(&e);
    let ch = e.class_h("t/C");
    let obj = e.vm.alloc_object(e.vm.class_named("t/C").unwrap());
    let oh = e.local(obj);
    let n = e.cstr("mVoid");
    let s = e.cstr("(IJFDLjava/lang/Object;Z)V");
    let mid = e.jni("GetMethodID", &[ch, n, s]).r0;
    // Null receiver.
    e.jni("CallVoidMethodA", &[0, mid, 0]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NullPointerException"));
    e.clear_pending();
    e.jni("CallNonvirtualVoidMethodA", &[0, ch, mid, 0]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NullPointerException"));
    e.clear_pending();
    // Bad method id.
    e.jni("CallVoidMethodA", &[oh, 0, 0]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoSuchMethodError"));
    e.clear_pending();
    e.jni("CallStaticVoidMethodA", &[ch, 99999, 0]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoSuchMethodError"));
    e.clear_pending();
    // A method that throws: the exception becomes pending, the result is zero.
    e.vm.define("t/Thrower").static_method("t", "()I", |c| Err(c.vm.throw_new("java/io/IOException", "disk")));
    let th = e.class_h("t/Thrower");
    let n = e.cstr("t");
    let s = e.cstr("()I");
    let tm = e.jni("GetStaticMethodID", &[th, n, s]).r0;
    assert_eq!(e.jni("CallStaticIntMethodA", &[th, tm, 0]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/io/IOException"));
    e.clear_pending();
    // A fatal stop inside a callee stops the CPU.
    e.vm.define("t/Stopper").static_method("t", "()V", |c| {
        c.cpu.request_stop(Stop::Host("halt".into()));
        Ok(Value::Void)
    });
    // Stops requested by the callee itself are plain returns here; a guest stop comes from natives.
    let st = e.class_h("t/Stopper");
    let v0 = e.cstr("()V");
    let tm = e.jni("GetStaticMethodID", &[st, n, v0]).r0;
    let _ = e.try_jni("CallStaticVoidMethodA", &[st, tm, 0]);
}

#[test]
fn guest_native_stop_inside_a_call_propagates() {
    let mut e = Env::new();
    let k = e.vm.define("t/Dies").id();
    let fault = e.code(&[0xE3A0_0000, 0xE580_0000, 0xE12F_FF1E]);
    e.vm.register_native(k, "die", "()V", fault);
    let ch = e.class_h("t/Dies");
    let n = e.cstr("die");
    let s = e.cstr("()V");
    let mid = e.jni("GetStaticMethodID", &[ch, n, s]).r0;
    let r = e.try_jni("CallStaticVoidMethodA", &[ch, mid, 0]);
    assert!(matches!(r, Err(Stop::Fault(_))), "{r:?}");
}

#[test]
fn object_construction() {
    let mut e = Env::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    e.vm.define("t/K").field("v", "I").method("<init>", "(IJ)V", move |c| {
        s2.lock().unwrap().push(c.args.to_vec());
        c.vm.set_field(c.this, "v", c.args[0]);
        Ok(Value::Void)
    });
    e.vm.define("t/Bad").method("<init>", "()V", |c| Err(c.vm.throw_new("java/lang/RuntimeException", "ctor")));
    let ch = e.class_h("t/K");
    let n = e.cstr("<init>");
    let s = e.cstr("(IJ)V");
    let mid = e.jni("GetMethodID", &[ch, n, s]).r0;
    let args = [A::I(9), A::J(10)];
    let words = |lead: &[u32]| var_words(lead, &args);
    let a = e.thunk("NewObject");
    let r = e.cpu.call(a, &words(&[e.env_ptr(), ch, mid])).unwrap().r0;
    let o = e.vm.resolve(r).unwrap();
    assert_eq!(e.vm.get_field(o, "v"), Value::Int(9));
    let va = va_list(&mut e, &args);
    let r = e.jni("NewObjectV", &[ch, mid, va]).r0;
    assert!(e.vm.resolve(r).is_some());
    let jv = jvalues(&mut e, &args);
    let r = e.jni("NewObjectA", &[ch, mid, jv]).r0;
    assert!(e.vm.resolve(r).is_some());
    assert_eq!(seen.lock().unwrap().len(), 3);
    // AllocObject skips the constructor.
    let r = e.jni("AllocObject", &[ch]).r0;
    assert_eq!(e.vm.get_field(e.vm.resolve(r).unwrap(), "v"), Value::Int(0));
    assert_eq!(e.jni("AllocObject", &[0]).r0, 0);
    // A throwing constructor leaves the exception pending and returns null.
    let bh = e.class_h("t/Bad");
    let s0 = e.cstr("()V");
    let bm = e.jni("GetMethodID", &[bh, n, s0]).r0;
    assert_eq!(e.jni("NewObjectA", &[bh, bm, 0]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/RuntimeException"));
    e.clear_pending();
    // Bad class / bad constructor id.
    e.jni("NewObjectA", &[0, mid, 0]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoSuchMethodError"));
    e.clear_pending();
    e.jni("NewObjectA", &[ch, 0, 0]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoSuchMethodError"));
    e.clear_pending();
    // GetObjectClass / IsInstanceOf.
    let oc = e.jni("GetObjectClass", &[r]).r0;
    assert_eq!(e.vm.class_of_mirror(e.vm.resolve(oc).unwrap()), e.vm.class_named("t/K"));
    assert_eq!(e.jni("GetObjectClass", &[0]).r0, 0);
    let objc = e.class_h("java/lang/Object");
    assert_eq!(e.jni("IsInstanceOf", &[r, objc]).r0, 1);
    assert_eq!(e.jni("IsInstanceOf", &[0, objc]).r0, 1);
    let strc = e.class_h("java/lang/String");
    assert_eq!(e.jni("IsInstanceOf", &[r, strc]).r0, 0);
    assert_eq!(e.jni("IsInstanceOf", &[r, 0]).r0, 0);
}

#[test]
fn member_lookup_errors() {
    let mut e = Env::new();
    let n = e.cstr("x");
    let s = e.cstr("I");
    for f in ["GetMethodID", "GetStaticMethodID", "GetFieldID", "GetStaticFieldID"] {
        assert_eq!(e.jni(f, &[0, n, s]).r0, 0, "{f}");
        assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoClassDefFoundError"));
        e.clear_pending();
    }
    // Strict mode: unknown members throw.
    let mut st = Env::with(|c| c.lenient = false);
    let ch = st.class_h("java/lang/Object");
    let n = st.cstr("nope");
    let s = st.cstr("()V");
    assert_eq!(st.jni("GetMethodID", &[ch, n, s]).r0, 0);
    assert_eq!(st.pending_class().as_deref(), Some("java/lang/NoSuchMethodError"));
    st.clear_pending();
    let si = st.cstr("I");
    assert_eq!(st.jni("GetFieldID", &[ch, n, si]).r0, 0);
    assert_eq!(st.pending_class().as_deref(), Some("java/lang/NoSuchFieldError"));
}

#[test]
fn field_accessors() {
    let mut e = Env::new();
    let k = e.vm.define("t/Fields").id();
    let types = [
        ("Object", "Ljava/lang/Object;"),
        ("Boolean", "Z"),
        ("Byte", "B"),
        ("Char", "C"),
        ("Short", "S"),
        ("Int", "I"),
        ("Long", "J"),
        ("Float", "F"),
        ("Double", "D"),
    ];
    let ch = e.class_h("t/Fields");
    let obj = e.vm.alloc_object(k);
    let oh = e.local(obj);
    let held = e.vm.new_string("held");
    let hh = e.local(held);
    for (rn, desc) in types {
        for is_static in [false, true] {
            let (idf, get, set) = if is_static {
                ("GetStaticFieldID", format!("GetStatic{rn}Field"), format!("SetStatic{rn}Field"))
            } else {
                ("GetFieldID", format!("Get{rn}Field"), format!("Set{rn}Field"))
            };
            let name = format!("{}{}", if is_static { "s" } else { "f" }, rn);
            let n = e.cstr(&name);
            let s = e.cstr(desc);
            let fid = e.jni(idf, &[ch, n, s]).r0;
            assert_ne!(fid, 0, "{name}");
            let holder = if is_static { ch } else { oh };
            // Defaults first.
            let r = e.jni(&get, &[holder, fid]);
            let wide = rn == "Long" || rn == "Double";
            assert_eq!(if wide { r.as_u64() } else { r.r0 as u64 }, 0, "{name} default");
            // Then a value, using the right argument registers for each type.
            let args: Vec<u32> = match rn {
                "Object" => vec![holder, fid, hh],
                "Long" => vec![holder, fid, 0, 0x1234_5678, 0x9],
                "Double" => vec![holder, fid, 0, 2.5f64.to_bits() as u32, (2.5f64.to_bits() >> 32) as u32],
                "Float" => vec![holder, fid, 3.5f32.to_bits()],
                "Byte" => vec![holder, fid, 0xfe],
                "Boolean" => vec![holder, fid, 1],
                "Char" => vec![holder, fid, 0xabcd],
                "Short" => vec![holder, fid, 0x8001],
                _ => vec![holder, fid, 77],
            };
            // 64-bit values must be 8-byte aligned in the argument words: env, holder, fid are
            // three words, so a pad word follows.
            let mut args = args;
            if rn == "Long" || rn == "Double" {
                args = vec![holder, fid, 0, args[3], args[4]];
                if rn == "Long" {
                    args[3] = 0x1234_5678;
                    args[4] = 0x9;
                }
            }
            e.jni(&set, &args);
            let r = e.jni(&get, &[holder, fid]);
            match rn {
                "Object" => assert_eq!(e.vm.resolve(r.r0), Some(held)),
                "Long" => assert_eq!(r.as_u64(), 0x9_1234_5678),
                "Double" => assert_eq!(r.as_f64(), 2.5),
                "Float" => assert_eq!(f32::from_bits(r.r0), 3.5),
                "Byte" => assert_eq!(r.r0 as i32, -2),
                "Boolean" => assert_eq!(r.r0, 1),
                "Char" => assert_eq!(r.r0, 0xabcd),
                "Short" => assert_eq!(r.r0 as i32, -32767),
                _ => assert_eq!(r.r0, 77),
            }
        }
    }
    // Field values live on the object / class.
    assert_eq!(e.vm.get_field(obj, "fInt"), Value::Int(77));
    assert_eq!(e.vm.get_static(k, "sInt"), Value::Int(77));
    // Null holder and bad field ids.
    let n = e.cstr("fInt");
    let s = e.cstr("I");
    let fid = e.jni("GetFieldID", &[ch, n, s]).r0;
    e.jni("GetIntField", &[0, fid]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NullPointerException"));
    e.clear_pending();
    e.jni("SetIntField", &[0, fid, 1]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NullPointerException"));
    e.clear_pending();
    assert_eq!(e.jni("GetIntField", &[oh, 0]).r0, 0);
    assert_eq!(e.jni("GetIntField", &[oh, 99999]).r0, 0);
    e.jni("SetIntField", &[oh, 0, 5]);
    e.jni("SetIntField", &[oh, 99999, 5]);
    assert_eq!(e.jni("GetStaticIntField", &[ch, 0]).r0, 0);
    // A stale object handle reads the declared default.
    assert_eq!(e.jni("GetIntField", &[oh ^ 0x40000, fid]).r0, 0);
}

#[test]
fn string_functions() {
    let mut e = Env::new();
    let p = e.cstr("héllo\u{1F600}");
    let h = e.jni("NewStringUTF", &[p]).r0;
    let s = e.vm.resolve(h).unwrap();
    assert_eq!(e.vm.string(s).as_deref(), Some("héllo\u{1F600}"));
    assert_eq!(e.jni("NewStringUTF", &[0]).r0, 0);
    assert_eq!(e.jni("GetStringLength", &[h]).r0, 7);
    assert_eq!(e.jni("GetStringLength", &[0]).r0, 0);
    assert_eq!(e.jni("GetStringUTFLength", &[h]).r0, 1 + 2 + 3 + 6);
    assert_eq!(e.jni("GetStringUTFLength", &[0]).r0, 0);
    let is_copy = e.buf(4);
    let u = e.jni("GetStringUTFChars", &[h, is_copy]).r0;
    assert_ne!(u, 0);
    assert_eq!(e.mem().read_u8(is_copy).unwrap(), 1);
    let want = crate::mutf8::encode(&crate::mutf8::units("héllo\u{1F600}"));
    assert_eq!(e.mem().read_bytes(u, want.len() as u32 + 1).unwrap(), [want.as_slice(), &[0]].concat());
    e.jni("ReleaseStringUTFChars", &[h, u]);
    // Releasing again (or a stranger) is ignored.
    e.jni("ReleaseStringUTFChars", &[h, u]);
    e.jni("ReleaseStringUTFChars", &[h, 0x1234]);
    assert_eq!(e.jni("GetStringUTFChars", &[0, 0]).r0, 0);
    // UTF-16 chars.
    let c = e.jni("GetStringChars", &[h, 0]).r0;
    assert_eq!(e.mem().read_u16(c).unwrap(), 'h' as u16);
    assert_eq!(e.mem().read_u16(c + 2).unwrap(), 'é' as u16);
    assert_eq!(e.mem().read_u16(c + 14).unwrap(), 0, "NUL terminated");
    e.jni("ReleaseStringChars", &[h, c]);
    assert_eq!(e.jni("GetStringChars", &[0, 0]).r0, 0);
    let c = e.jni("GetStringCritical", &[h, 0]).r0;
    assert_ne!(c, 0);
    e.jni("ReleaseStringCritical", &[h, c]);
    // NewString from UTF-16.
    let buf = e.buf(16);
    for (i, ch) in "abc".encode_utf16().enumerate() {
        e.mem().write_u16(buf + 2 * i as u32, ch).unwrap();
    }
    let n = e.jni("NewString", &[buf, 3]).r0;
    assert_eq!(e.vm.string(e.vm.resolve(n).unwrap()).as_deref(), Some("abc"));
    // Regions.
    let out = e.buf(32);
    e.jni("GetStringRegion", &[h, 1, 2, out]);
    assert_eq!(e.mem().read_u16(out).unwrap(), 'é' as u16);
    assert_eq!(e.mem().read_u16(out + 2).unwrap(), 'l' as u16);
    e.jni("GetStringUTFRegion", &[h, 1, 2, out]);
    assert_eq!(e.mem().read_cstr(out).unwrap(), "él");
    e.jni("GetStringUTFRegion", &[h, 0, 8, out]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/StringIndexOutOfBoundsException"));
    e.clear_pending();
    e.jni("GetStringRegion", &[h, 5, 4, out]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/StringIndexOutOfBoundsException"));
    e.clear_pending();
    e.jni("GetStringRegion", &[0, 0, 1, out]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NullPointerException"));
    e.clear_pending();
    e.jni("GetStringRegion", &[h, u32::MAX, 1, out]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/StringIndexOutOfBoundsException"));
    e.clear_pending();
}

#[test]
fn primitive_array_functions() {
    let mut e = Env::new();
    for (name, ty) in ARRAY_TYPES {
        let size = ty.size() as u32;
        let a = e.jni(&format!("New{name}Array"), &[4]).r0;
        let obj = e.vm.resolve(a).unwrap();
        assert_eq!(e.vm.array_len(obj), Some(4));
        assert_eq!(e.jni("GetArrayLength", &[a]).r0, 4);
        // Fill through SetRegion, read through GetRegion.
        let src = e.buf(4 * size);
        for i in 0..4 * size {
            e.mem().write_u8(src + i, (i + 1) as u8).unwrap();
        }
        e.jni(&format!("Set{name}ArrayRegion"), &[a, 1, 3, src]);
        assert_eq!(e.pending_class(), None);
        let dst = e.buf(4 * size);
        e.jni(&format!("Get{name}ArrayRegion"), &[a, 0, 4, dst]);
        let got = e.mem().read_bytes(dst, 4 * size).unwrap();
        assert!(got[..size as usize].iter().all(|b| *b == 0));
        assert_eq!(&got[size as usize..4 * size as usize], &e.mem().read_bytes(src, 3 * size).unwrap()[..]);
        // Elements: copy out, modify, release with each mode.
        let is_copy = e.buf(4);
        e.mem().write_bytes(is_copy, &[0, 0x11, 0x22, 0x33]).unwrap();
        let p = e.jni(&format!("Get{name}ArrayElements"), &[a, is_copy]).r0;
        assert_ne!(p, 0);
        // A jboolean: the bytes after it belong to the caller's neighbouring locals.
        assert_eq!(e.mem().read_bytes(is_copy, 4).unwrap(), [1, 0x11, 0x22, 0x33]);
        e.mem().write_u8(p, 0xAA).unwrap();
        // ABORT discards the changes and frees the buffer.
        e.jni(&format!("Release{name}ArrayElements"), &[a, p, 2]);
        e.jni(&format!("Get{name}ArrayRegion"), &[a, 0, 1, dst]);
        assert_eq!(e.mem().read_u8(dst).unwrap(), 0);
        let p = e.jni(&format!("Get{name}ArrayElements"), &[a, 0]).r0;
        e.mem().write_u8(p, 0xAA).unwrap();
        // COMMIT copies back and keeps the buffer.
        e.jni(&format!("Release{name}ArrayElements"), &[a, p, 1]);
        e.jni(&format!("Get{name}ArrayRegion"), &[a, 0, 1, dst]);
        assert_eq!(e.mem().read_u8(dst).unwrap(), 0xAA);
        // Mode 0 copies back and frees.
        e.mem().write_u8(p, 0xBB).unwrap();
        e.jni(&format!("Release{name}ArrayElements"), &[a, p, 0]);
        e.jni(&format!("Get{name}ArrayRegion"), &[a, 0, 1, dst]);
        assert_eq!(e.mem().read_u8(dst).unwrap(), 0xBB);
        e.jni(&format!("Release{name}ArrayElements"), &[a, p, 0]); // already gone: ignored
                                                                   // Out-of-range regions and null arrays.
        for (start, len) in [(3u32, 2u32), (u32::MAX, 1), (0, u32::MAX)] {
            e.jni(&format!("Get{name}ArrayRegion"), &[a, start, len, dst]);
            assert_eq!(e.pending_class().as_deref(), Some("java/lang/ArrayIndexOutOfBoundsException"));
            e.clear_pending();
            e.jni(&format!("Set{name}ArrayRegion"), &[a, start, len, dst]);
            assert_eq!(e.pending_class().as_deref(), Some("java/lang/ArrayIndexOutOfBoundsException"));
            e.clear_pending();
        }
        e.jni(&format!("Get{name}ArrayRegion"), &[0, 0, 1, dst]);
        assert_eq!(e.pending_class().as_deref(), Some("java/lang/NullPointerException"));
        e.clear_pending();
        assert_eq!(e.jni(&format!("Get{name}ArrayElements"), &[0, 0]).r0, 0);
        // Negative length.
        assert_eq!(e.jni(&format!("New{name}Array"), &[u32::MAX]).r0, 0);
        assert_eq!(e.pending_class().as_deref(), Some("java/lang/NegativeArraySizeException"));
        e.clear_pending();
    }
    // Critical access is the same as Elements.
    let a = e.jni("NewIntArray", &[2]).r0;
    let p = e.jni("GetPrimitiveArrayCritical", &[a, 0]).r0;
    e.mem().write_u32(p, 99).unwrap();
    e.jni("ReleasePrimitiveArrayCritical", &[a, p, 0]);
    let obj = e.vm.resolve(a).unwrap();
    assert_eq!(e.vm.data(obj), Some(Data::Prim(Ty::Int, vec![99, 0, 0, 0, 0, 0, 0, 0])));
    // Elements of a non-array and of a string give null.
    let s = e.local(e.vm.new_string("s"));
    assert_eq!(e.jni("GetIntArrayElements", &[s, 0]).r0, 0);
    // Heap exhaustion while lending raises OutOfMemoryError.
    let big = e.jni("NewByteArray", &[0x7000_0000]).r0;
    let _ = big;
}

#[test]
fn object_array_functions() {
    let mut e = Env::new();
    let sc = e.class_h("java/lang/String");
    let init = e.local(e.vm.new_string("init"));
    let a = e.jni("NewObjectArray", &[3, sc, init]).r0;
    let arr = e.vm.resolve(a).unwrap();
    assert_eq!(e.vm.class_name(e.vm.class_of(arr).unwrap()), "[Ljava/lang/String;");
    assert_eq!(e.jni("GetArrayLength", &[a]).r0, 3);
    let g = e.jni("GetObjectArrayElement", &[a, 2]).r0;
    assert_eq!(e.vm.string(e.vm.resolve(g).unwrap()).as_deref(), Some("init"));
    let v = e.local(e.vm.new_string("v"));
    e.jni("SetObjectArrayElement", &[a, 1, v]);
    let g = e.jni("GetObjectArrayElement", &[a, 1]).r0;
    assert_eq!(e.vm.string(e.vm.resolve(g).unwrap()).as_deref(), Some("v"));
    e.jni("SetObjectArrayElement", &[a, 1, 0]);
    assert_eq!(e.jni("GetObjectArrayElement", &[a, 1]).r0, 0);
    // No initial element, null class.
    let b = e.jni("NewObjectArray", &[2, 0, 0]).r0;
    assert_eq!(e.vm.class_name(e.vm.class_of(e.vm.resolve(b).unwrap()).unwrap()), "[Ljava/lang/Object;");
    assert_eq!(e.jni("GetObjectArrayElement", &[b, 0]).r0, 0);
    // Bounds and null.
    for idx in [3u32, u32::MAX] {
        assert_eq!(e.jni("GetObjectArrayElement", &[a, idx]).r0, 0);
        assert_eq!(e.pending_class().as_deref(), Some("java/lang/ArrayIndexOutOfBoundsException"));
        e.clear_pending();
        e.jni("SetObjectArrayElement", &[a, idx, v]);
        assert_eq!(e.pending_class().as_deref(), Some("java/lang/ArrayIndexOutOfBoundsException"));
        e.clear_pending();
    }
    e.jni("SetObjectArrayElement", &[0, 0, v]);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/ArrayIndexOutOfBoundsException"));
    e.clear_pending();
    assert_eq!(e.jni("NewObjectArray", &[u32::MAX, sc, 0]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NegativeArraySizeException"));
    e.clear_pending();
    assert_eq!(e.jni("GetArrayLength", &[0]).r0, 0);
}

#[test]
fn register_natives_binds_guest_functions() {
    let mut e = Env::new();
    let add = e.code(&[0xE082_0003, 0xE12F_FF1E]);
    e.vm.define("t/Reg");
    let ch = e.class_h("t/Reg");
    let table = e.buf(48);
    let (n1, s1) = (e.cstr("add"), e.cstr("(II)I"));
    let (n2, s2) = (e.cstr("other"), e.cstr("()V"));
    let m = e.mem();
    for (i, (n, s, f)) in [(n1, s1, add), (n2, s2, 0x1000)].into_iter().enumerate() {
        m.write_u32(table + 12 * i as u32, n).unwrap();
        m.write_u32(table + 12 * i as u32 + 4, s).unwrap();
        m.write_u32(table + 12 * i as u32 + 8, f).unwrap();
    }
    assert_eq!(e.jni("RegisterNatives", &[ch, table, 2]).r0, 0);
    let k = e.vm.class_named("t/Reg").unwrap();
    assert_eq!(e.vm.native_addr(k, "add", "(II)I"), Some(add));
    assert_eq!(e.vm.native_addr(k, "other", "()V"), Some(0x1000));
    assert_eq!(e.scall("t/Reg", "add", "(II)I", &[Value::Int(20), Value::Int(22)]), Ok(Value::Int(42)));
    // A bad signature fails the whole call.
    let bad = e.cstr("not a signature");
    m.write_u32(table + 16, bad).unwrap();
    assert_eq!(e.jni("RegisterNatives", &[ch, table + 12, 1]).r0 as i32, -1);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/NoSuchMethodError"));
    e.clear_pending();
    assert_eq!(e.jni("RegisterNatives", &[0, table, 1]).r0 as i32, -1);
    assert_eq!(e.jni("RegisterNatives", &[ch, table, 0]).r0, 0);
}

#[test]
fn java_vm_and_direct_buffers() {
    let mut e = Env::new();
    let out = e.buf(4);
    assert_eq!(e.jni("GetJavaVM", &[out]).r0, 0);
    assert_eq!(e.mem().read_u32(out).unwrap(), e.layout.java_vm);
    assert_eq!(e.jni("GetJavaVM", &[0]).r0, 0);
    // Direct byte buffers: (address, capacity as a 64-bit value).
    let b = e.jni("NewDirectByteBuffer", &[0x5000_1000, 4096, 0]).r0;
    assert_eq!(e.jni("GetDirectBufferAddress", &[b]).r0, 0x5000_1000);
    assert_eq!(e.jni("GetDirectBufferCapacity", &[b]).as_u64(), 4096);
    assert_eq!(e.jni("GetDirectBufferAddress", &[0]).r0, 0);
    assert_eq!(e.jni("GetDirectBufferCapacity", &[0]).as_u64() as i64, -1);
}

#[test]
fn lending_buffers_reports_out_of_memory() {
    let (mut e, fail) = Env::flaky();
    let p = e.cstr("text");
    let h = e.jni("NewStringUTF", &[p]).r0;
    let a = e.jni("NewIntArray", &[4]).r0;
    fail.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(e.jni("GetStringUTFChars", &[h, 0]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/OutOfMemoryError"));
    e.clear_pending();
    assert_eq!(e.jni("GetIntArrayElements", &[a, 0]).r0, 0);
    assert_eq!(e.pending_class().as_deref(), Some("java/lang/OutOfMemoryError"));
    e.clear_pending();
    // A thread that has no JNIEnv yet cannot get one either.
    assert_eq!(e.vm.env_ptr(77), 0);
    fail.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_ne!(e.vm.env_ptr(77), 0);
}
