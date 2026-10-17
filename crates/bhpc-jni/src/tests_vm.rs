use crate::tests::{Env, SCRATCH};
use crate::value::{ClassId, Ty};
use crate::vm::{Imp, Kind, RefKind};
use crate::*;
use bhpc_cpu::Stop;
use std::sync::{Arc, Mutex};

fn strict() -> Env {
    Env::with(|c| c.lenient = false)
}

#[test]
fn env_boots() {
    let e = Env::new();
    assert!(e.vm.class_named("java/lang/String").is_some());
    assert_ne!(e.env_ptr(), 0);
    assert_eq!(e.vm.layout(), Some(e.layout));
    assert!(e.vm.platform().screen_width == 800);
    assert!(e.vm.assets().read("hello.txt").is_some());
    assert!(e.vm.mem().is_mapped(SCRATCH, 4));
}

#[test]
fn classes_and_hierarchy() {
    let e = Env::new();
    let vm = &e.vm;
    let a = vm.define("t/A").payload(|| Data::List(vec![])).id();
    let i = vm.define("t/I").interface().id();
    let b = vm.define("t/B").extends("t/A").implements(&["t/I"]).id();
    assert_eq!(vm.class_named("t.B"), Some(b));
    assert_eq!(vm.class_name(b), "t/B");
    assert_eq!(vm.superclass(b), Some(a));
    assert!(vm.is_assignable(b, a) && vm.is_assignable(b, i) && vm.is_assignable(b, b));
    assert!(!vm.is_assignable(a, b) && !vm.is_assignable(a, i));
    let obj = vm.class_named("java/lang/Object").unwrap();
    assert!(vm.is_assignable(i, obj) && vm.is_assignable(b, obj));
    assert_eq!(vm.superclass(obj), None);
    // Payload is inherited from the superclass.
    let o = vm.alloc_object(b);
    assert_eq!(vm.data(o), Some(Data::List(vec![])));
    assert!(vm.instance_of(o, "t/I") && vm.instance_of(o, "t/A") && !vm.instance_of(o, "java/lang/String"));
    assert!(!vm.instance_of(Obj(9999), "t/A"));
    assert!(!vm.instance_of(o, "no/such/Class"));
    // Mirrors.
    let m = vm.class_object(b);
    assert_eq!(vm.class_object(b), m);
    assert_eq!(vm.class_of_mirror(m), Some(b));
    assert_eq!(vm.class_of_mirror(o), None);
    assert_eq!(vm.class_of_mirror(Obj(9999)), None);
    // Arrays are classes too.
    let arr = vm.class_named("[Lt/A;").unwrap();
    assert_eq!(vm.class_name(arr), "[Lt/A;");
    // Placeholders are not defined classes until `define` runs.
    vm.define("t/Child").extends("t/Later");
    assert_eq!(vm.class_named("t/Later"), None);
    vm.define("t/Later");
    assert!(vm.class_named("t/Later").is_some());
}

#[test]
fn find_class_lenient_and_strict() {
    let e = Env::new();
    let c = e.vm.find_class("com.unknown.Thing").unwrap();
    assert_eq!(e.vm.class_name(c), "com/unknown/Thing");
    assert_eq!(e.vm.find_class("com/unknown/Thing").unwrap(), c);
    assert!(e.log_text().contains("stub class com/unknown/Thing"));
    assert_eq!(e.vm.report().stub_classes, ["com/unknown/Thing"]);

    let s = strict();
    let err = s.vm.find_class("com/unknown/Thing").unwrap_err();
    let Throw::Exc(x) = err else { panic!() };
    assert_eq!(s.vm.exception_message(x).as_deref(), Some("com/unknown/Thing"));
    assert!(s.vm.instance_of(x, "java/lang/NoClassDefFoundError"));
    // An unknown exception class falls back to Object.
    let x = s.vm.new_exception("no/Such", "m");
    assert_eq!(s.vm.class_of(x), Some(ClassId(0)));
}

#[test]
fn strings_and_arrays() {
    let e = Env::new();
    let vm = &e.vm;
    let s = vm.new_string("héllo");
    assert_eq!(vm.string(s).as_deref(), Some("héllo"));
    assert_eq!(vm.string_units(s).unwrap().len(), 5);
    assert_eq!(vm.string(Obj(9999)), None);
    let a = vm.new_prim_array(Ty::Int, 3);
    assert_eq!(vm.array_len(a), Some(3));
    assert_eq!(vm.string(a), None);
    assert_eq!(vm.string_units(a), None);
    let b = vm.new_byte_array(&[1, 2, 3, 4]);
    assert_eq!(vm.array_len(b), Some(4));
    assert_eq!(vm.data(b), Some(Data::Prim(Ty::Byte, vec![1, 2, 3, 4])));
    let o = vm.new_object_array("java/lang/String", 2);
    assert_eq!(vm.array_len(o), Some(2));
    assert_eq!(vm.class_name(vm.class_of(o).unwrap()), "[Ljava/lang/String;");
    let oo = vm.new_object_array("[I", 1);
    assert_eq!(vm.class_name(vm.class_of(oo).unwrap()), "[[I");
    let sa = vm.new_string_array(&["a".to_string(), "b".to_string()]);
    let Some(Data::Objs(items)) = vm.data(sa) else { panic!() };
    assert_eq!(items.len(), 2);
    assert_eq!(vm.string(items[1].unwrap()).as_deref(), Some("b"));
    assert_eq!(vm.array_len(s), None);
    assert_eq!(vm.data(Obj(0)), None);
    let d = vm.new_data_object("t/Holder", Data::Bytes { data: vec![1], pos: 0 });
    assert_eq!(vm.data(d), Some(Data::Bytes { data: vec![1], pos: 0 }));
}

#[test]
fn boxes_and_equality() {
    let e = Env::new();
    let vm = &e.vm;
    let cases = [
        (Ty::Bool, Value::Int(1), "java/lang/Boolean"),
        (Ty::Byte, Value::Int(-1), "java/lang/Byte"),
        (Ty::Char, Value::Int(65), "java/lang/Character"),
        (Ty::Short, Value::Int(7), "java/lang/Short"),
        (Ty::Int, Value::Int(42), "java/lang/Integer"),
        (Ty::Long, Value::Long(1 << 40), "java/lang/Long"),
        (Ty::Float, Value::Float(1.5), "java/lang/Float"),
        (Ty::Double, Value::Double(2.5), "java/lang/Double"),
    ];
    for (ty, v, class) in cases {
        let b = vm.box_value(v, ty).unwrap();
        assert_eq!(vm.class_name(vm.class_of(b).unwrap()), class);
        assert_eq!(vm.unbox(b), Some(v));
        let b2 = vm.box_value(v, ty).unwrap();
        assert!(vm.equals(Some(b), Some(b2)));
        assert!(vm.equals(Some(b), Some(b)));
    }
    assert!(vm.box_value(Value::Void, Ty::Void).is_none());
    assert!(vm.box_value(Value::Void, Ty::Ref).is_none());
    let i = vm.box_value(Value::Int(1), Ty::Int).unwrap();
    let l = vm.box_value(Value::Int(1), Ty::Long).unwrap();
    let j = vm.box_value(Value::Int(2), Ty::Int).unwrap();
    assert!(!vm.equals(Some(i), Some(l)));
    assert!(!vm.equals(Some(i), Some(j)));
    assert!(vm.unbox(vm.new_string("x")).is_none());
    assert!(vm.unbox(Obj(9999)).is_none());
    assert!(vm.equals(None, None));
    assert!(!vm.equals(None, Some(i)));
    let (s1, s2, s3) = (vm.new_string("a"), vm.new_string("a"), vm.new_string("b"));
    assert!(vm.equals(Some(s1), Some(s2)));
    assert!(!vm.equals(Some(s1), Some(s3)));
    let plain = vm.alloc_object(vm.class_named("java/lang/Object").unwrap());
    let plain2 = vm.alloc_object(vm.class_named("java/lang/Object").unwrap());
    assert!(!vm.equals(Some(plain), Some(plain2)));
    // A class that merely looks like a box is not one.
    let fake = vm.alloc_object(vm.define("java/lang/Fake").id());
    assert!(vm.unbox(fake).is_none());
}

#[test]
fn fields_by_name() {
    let e = Env::new();
    let vm = &e.vm;
    let k = vm.define("t/F").field("a", "I").static_field("S", "J", Value::Long(5)).id();
    let o = vm.alloc_object(k);
    assert_eq!(vm.get_field(o, "a"), Value::Int(0));
    vm.set_field(o, "a", Value::Int(9));
    assert_eq!(vm.get_field(o, "a"), Value::Int(9));
    // Unknown fields are declared on first write with the value's kind.
    for (n, v) in [
        ("l", Value::Long(1)),
        ("f", Value::Float(1.0)),
        ("d", Value::Double(1.0)),
        ("r", Value::Ref(Some(o))),
        ("i", Value::Int(2)),
    ] {
        vm.set_field(o, n, v);
        assert_eq!(vm.get_field(o, n), v);
    }
    assert_eq!(vm.get_field(o, "nope"), Value::Int(0));
    assert_eq!(vm.get_field(Obj(9999), "a"), Value::Int(0));
    vm.set_field(Obj(9999), "a", Value::Int(1));
    assert_eq!(vm.get_static(k, "S"), Value::Long(5));
    vm.set_static(k, "S", Value::Long(6));
    assert_eq!(vm.get_static(k, "S"), Value::Long(6));
    assert_eq!(vm.get_static(k, "nope"), Value::Int(0));
    vm.set_static(k, "fresh", Value::Int(3));
    assert_eq!(vm.get_static(k, "fresh"), Value::Int(3));
    assert_eq!(vm.field_id(k, "a", "I"), vm.field_id(k, "a", "I"));
    // Subclasses see inherited fields; the declared default type is used for unset ones.
    let sub = vm.define("t/G").extends("t/F").id();
    let g = vm.alloc_object(sub);
    vm.set_field(g, "a", Value::Int(1));
    assert_eq!(vm.get_field(g, "a"), Value::Int(1));
}

#[test]
fn singleton_is_created_once() {
    let e = Env::new();
    let mut n = 0;
    let a = e.vm.singleton("x", |vm| {
        n += 1;
        vm.new_string("s")
    });
    let b = e.vm.singleton("x", |_| panic!("not again"));
    assert_eq!(a, b);
    assert_eq!(n, 1);
}

#[test]
fn references_and_frames() {
    let e = Env::new();
    let vm = &e.vm;
    let pid = e.cpu.processor_id();
    let o = vm.new_string("x");
    assert_eq!(vm.new_ref(pid, None, RefKind::Local), 0);
    let l = vm.new_ref(pid, Some(o), RefKind::Local);
    let g = vm.new_ref(pid, Some(o), RefKind::Global);
    let w = vm.new_ref(pid, Some(o), RefKind::Weak);
    assert_ne!(l, 0);
    assert_eq!((vm.ref_type(l), vm.ref_type(g), vm.ref_type(w)), (1, 2, 3));
    assert_eq!(vm.resolve(l), Some(o));
    assert_eq!(vm.resolve(0), None);
    assert_eq!(vm.resolve(0x7777_0000), None);
    assert_eq!(vm.resolve(l ^ 2), None, "kind mismatch");
    assert_eq!(vm.ref_type(0), 0);
    assert_eq!(vm.ref_type(l ^ 2), 0);
    assert_eq!(vm.live_refs(), 3);
    vm.delete_ref(pid, g);
    vm.delete_ref(pid, g);
    // A thread that never made a local reference has no thread state to clean.
    let foreign = vm.new_ref(pid, Some(o), RefKind::Global);
    vm.delete_ref(4242, foreign);
    assert_eq!(vm.resolve(foreign), None);
    vm.delete_ref(pid, 0x7777_0000);
    assert_eq!(vm.resolve(g), None);
    assert_eq!(vm.live_refs(), 2);
    // Slots are reused.
    let g2 = vm.new_ref(pid, Some(o), RefKind::Global);
    assert_eq!(g2 >> 3, g >> 3);
    // Frames release their locals and can hand one result up.
    vm.push_frame(pid);
    let inner = vm.new_ref(pid, Some(o), RefKind::Local);
    let inner2 = vm.new_ref(pid, Some(vm.new_string("y")), RefKind::Local);
    let kept = vm.pop_frame(pid, Some(vm.resolve(inner2).unwrap()));
    assert_eq!(vm.resolve(inner), None);
    assert_eq!(vm.string(vm.resolve(kept).unwrap()).as_deref(), Some("y"));
    // Popping with no frame is harmless.
    assert_eq!(vm.pop_frame(pid, None), 0);
    assert!(vm.resolve(l).is_some(), "outer locals survive");
}

#[test]
fn gc_keeps_reachable_objects() {
    let e = Env::new();
    let vm = &e.vm;
    let pid = e.cpu.processor_id();
    let keep_ref = vm.new_string("ref");
    let r = vm.new_ref(pid, Some(keep_ref), RefKind::Global);
    let pinned = vm.new_string("pinned");
    vm.pin(pinned);
    let in_list = vm.new_string("in list");
    let list = vm.new_data_object("java/util/ArrayList", Data::List(vec![Some(in_list)]));
    let in_map_k = vm.new_string("k");
    let in_map_v = vm.new_string("v");
    let map = vm.new_data_object("java/util/HashMap", Data::Map(vec![(Some(in_map_k), Some(in_map_v))]));
    let in_queue = vm.new_string("q");
    let queue = vm.new_data_object("android/os/MessageQueue", Data::Queue(vec![in_queue]));
    let in_arr = vm.new_string("arr");
    let arr = vm.new_object_array("java/lang/String", 1);
    vm.with_data(arr, |d| *d = Data::Objs(vec![Some(in_arr)]));
    let holder_val = vm.new_string("field");
    let holder = vm.alloc_object(vm.define("t/H").field("f", "Ljava/lang/Object;").id());
    vm.set_field(holder, "f", Value::Ref(Some(holder_val)));
    let static_val = vm.new_string("static");
    vm.define("t/H").static_field("S", "Ljava/lang/Object;", Value::Ref(Some(static_val)));
    let roots = [list, map, queue, arr, holder].map(|o| vm.new_ref(pid, Some(o), RefKind::Global));
    let garbage = vm.new_string("garbage");
    let garbage2 = vm.new_data_object("java/util/ArrayList", Data::List(vec![Some(garbage)]));
    let before = vm.live_objects();
    let freed = vm.gc();
    assert_eq!(freed, 2);
    assert_eq!(vm.live_objects(), before - 2);
    for o in [keep_ref, pinned, in_list, in_map_k, in_map_v, in_queue, in_arr, holder_val, static_val] {
        assert!(vm.string(o).is_some(), "{o:?} must survive");
    }
    assert_eq!(vm.string(garbage), None);
    assert_eq!(vm.data(garbage2), None);
    assert_eq!(vm.gc(), 0);
    // Cycles and shared objects are marked once.
    let cyc = vm.new_data_object("java/util/ArrayList", Data::List(vec![]));
    vm.with_data(cyc, |d| *d = Data::List(vec![Some(cyc), Some(cyc)]));
    vm.pin(cyc);
    assert_eq!(vm.gc(), 0);
    // Freed slots are reused.
    let again = vm.new_string("again");
    assert!(again == garbage || again == garbage2);
    let _ = (r, roots);
}

#[test]
fn exceptions() {
    let e = Env::new();
    let vm = &e.vm;
    let x = vm.new_exception("java/lang/IllegalStateException", "boom");
    assert_eq!(vm.exception_message(x).as_deref(), Some("boom"));
    assert_eq!(vm.exception_message(vm.new_string("not an exception")), None);
    let Throw::Exc(y) = vm.throw_new("java/io/IOException", "io") else { panic!() };
    assert!(vm.instance_of(y, "java/lang/Exception"));
    let pid = e.cpu.processor_id();
    assert_eq!(vm.pending(pid), None);
    assert_eq!(vm.take_pending(pid), None);
    vm.set_pending(pid, x);
    assert_eq!(vm.pending(pid), Some(x));
    assert_eq!(vm.take_pending(pid), Some(x));
    assert_eq!(vm.pending(pid), None);
}

#[test]
fn method_lookup_lenient() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    let i = vm.define("t/Iface").interface().method("im", "()I", |_| Ok(Value::Int(1))).id();
    let a = vm.define("t/Base").method("bm", "()I", |_| Ok(Value::Int(2))).id();
    let b = vm.define("t/Impl").extends("t/Base").implements(&["t/Iface"]).id();
    let mi = vm.method_id(b, "im", "()I", false).unwrap();
    let mb = vm.method_id(b, "bm", "()I", false).unwrap();
    assert_ne!(mi, mb);
    assert_eq!(vm.method_id(a, "bm", "()I", false).unwrap(), mb);
    assert_eq!(vm.method_info(mi).unwrap().3, "im");
    assert!(vm.method_info(0).is_none());
    assert!(vm.method_info(99999).is_none());
    // Unknown member: stub (logged), found again later.
    let stub = vm.method_id(b, "gone", "(I)J", true).unwrap();
    assert_eq!(vm.method_id(b, "gone", "(I)J", true).unwrap(), stub);
    assert!(e.log_text().contains("stub method t/Impl.gone(I)J"));
    let o = vm.alloc_object(b);
    let vm2 = vm.clone();
    assert_eq!(vm2.invoke(&mut e.cpu, stub, Recv::Static(b), &[Value::Int(1)], false), Ok(Value::Long(0)));
    // Bad descriptor.
    let Err(Throw::Exc(x)) = vm.method_id(b, "x", "nonsense", false) else { panic!() };
    assert!(vm.instance_of(x, "java/lang/NoSuchMethodError"));
    // Fields.
    vm.define("t/Fl").field("a", "I");
    let k = vm.class_named("t/Fl").unwrap();
    let fa = vm.jfield_id(k, "a", "I").unwrap();
    assert_eq!(vm.jfield_id(k, "a", "I").unwrap(), fa);
    let fb = vm.jfield_id(k, "b", "J").unwrap();
    assert_ne!(fa, fb);
    assert!(e.log_text().contains("stub field t/Fl.b J"));
    let Err(Throw::Exc(x)) = vm.jfield_id(k, "c", "V") else { panic!() };
    assert!(vm.instance_of(x, "java/lang/NoSuchFieldError"));
    assert!(vm.is_assignable(b, i));
    let _ = o;
}

#[test]
fn method_lookup_strict() {
    let s = strict();
    let k = s.vm.define("t/S").field("a", "I").id();
    let Err(Throw::Exc(x)) = s.vm.method_id(k, "nope", "()V", false) else { panic!() };
    assert_eq!(s.vm.exception_message(x).as_deref(), Some("t/S.nope()V"));
    let Err(Throw::Exc(x)) = s.vm.jfield_id(k, "nope", "I") else { panic!() };
    assert!(s.vm.instance_of(x, "java/lang/NoSuchFieldError"));
    assert!(s.vm.jfield_id(k, "a", "I").is_ok());
}

#[test]
fn invoke_dispatch() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let c1 = calls.clone();
    let c2 = calls.clone();
    vm.define("t/Base")
        .method("who", "()I", move |c| {
            c1.lock().unwrap().push(("base", c.class));
            Ok(Value::Int(1))
        })
        .method("echo", "(IJFD)D", |c| {
            Ok(Value::Double(c.int(0) as f64 + c.long(1) as f64 + c.float(2) as f64 + c.double(3)))
        })
        .static_method("st", "(Ljava/lang/String;)Ljava/lang/String;", |c| Ok(Value::Ref(c.obj(0))));
    vm.define("t/Derived").extends("t/Base").method("who", "()I", move |c| {
        c2.lock().unwrap().push(("derived", c.class));
        Ok(Value::Int(2))
    });
    let derived = vm.class_named("t/Derived").unwrap();
    let base = vm.class_named("t/Base").unwrap();
    let o = vm.alloc_object(derived);
    // Virtual dispatch finds the override; non-virtual does not.
    let mid = vm.method_id(base, "who", "()I", false).unwrap();
    assert_eq!(vm.invoke(&mut e.cpu, mid, Recv::Instance(o), &[], true), Ok(Value::Int(2)));
    assert_eq!(vm.invoke(&mut e.cpu, mid, Recv::Instance(o), &[], false), Ok(Value::Int(1)));
    assert_eq!(calls.lock().unwrap().as_slice(), [("derived", derived), ("base", derived)]);
    assert_eq!(vm.call_method(&mut e.cpu, o, "who", "()I", &[]), Ok(Value::Int(2)));
    assert_eq!(
        vm.call_method(
            &mut e.cpu,
            o,
            "echo",
            "(IJFD)D",
            &[Value::Int(1), Value::Long(2), Value::Float(0.5), Value::Double(0.25)]
        ),
        Ok(Value::Double(3.75))
    );
    let s = e.string("hi");
    assert_eq!(vm.call_static(&mut e.cpu, "t/Base", "st", "(Ljava/lang/String;)Ljava/lang/String;", &[s]), Ok(s));
    // Argument accessors tolerate missing arguments.
    vm.define("t/Args").static_method("none", "()I", |c| {
        assert_eq!((c.int(3), c.long(3), c.float(3), c.double(3), c.obj(3), c.string(3)), (0, 0, 0.0, 0.0, None, None));
        Ok(Value::Int(7))
    });
    assert_eq!(vm.call_static(&mut e.cpu, "t/Args", "none", "()I", &[]), Ok(Value::Int(7)));
    // Null receivers and bad ids.
    assert!(matches!(vm.invoke(&mut e.cpu, mid, Recv::Instance(Obj(9999)), &[], true), Err(Throw::Exc(_))));
    assert!(matches!(vm.call_method(&mut e.cpu, Obj(9999), "who", "()I", &[]), Err(Throw::Exc(_))));
    assert!(matches!(vm.invoke(&mut e.cpu, 0, Recv::Static(base), &[], false), Err(Throw::Exc(_))));
    assert!(matches!(vm.invoke(&mut e.cpu, 123456, Recv::Static(base), &[], false), Err(Throw::Exc(_))));
    assert!(matches!(vm.invoke(&mut e.cpu, 123456, Recv::Instance(o), &[], true), Err(Throw::Exc(_))));
    // new_object runs the constructor.
    vm.define("t/Ctor").field("v", "I").method("<init>", "(I)V", |c| {
        c.vm.set_field(c.this, "v", c.args[0]);
        Ok(Value::Void)
    });
    let n = vm.new_object(&mut e.cpu, "t/Ctor", "(I)V", &[Value::Int(5)]).unwrap();
    assert_eq!(vm.get_field(n, "v"), Value::Int(5));
    assert!(vm.new_object(&mut e.cpu, "t/Ctor", "(Z)V", &[Value::Int(5)]).is_ok(), "lenient stub constructor");
    let st = strict();
    st.vm.define("t/NoCtor");
    let mut st = st;
    let vm = st.vm.clone();
    assert!(vm.new_object(&mut st.cpu, "t/NoCtor", "(I)V", &[Value::Int(1)]).is_err());
}

#[test]
fn native_binding_and_calls() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    let k = vm.define("t/N").native("known", "(II)I", true).id();
    // add: r0 = r2 + r3
    let add = e.code(&[0xE082_0003, 0xE12F_FF1E]);
    // echo long: r0:r1 = r2:r3
    let echo = e.code(&[0xE1A0_0002, 0xE1A0_1003, 0xE12F_FF1E]);
    // echo float: r0 = r2
    let echof = e.code(&[0xE1A0_0002, 0xE12F_FF1E]);
    // echo double (aligned in r2:r3): same
    // return the second 32-bit arg (r3) sign-extended from byte/short/char/bool via and masks
    let ret_r3 = e.code(&[0xE1A0_0003, 0xE12F_FF1E]);
    assert_eq!(vm.native_addr(k, "known", "(II)I"), None);
    assert!(vm.register_native(k, "known", "(II)I", add));
    assert_eq!(vm.native_addr(k, "known", "(II)I"), Some(add));
    assert!(vm.register_native(k, "echo", "(J)J", echo));
    assert!(vm.register_native(k, "echof", "(F)F", echof));
    assert!(vm.register_native(k, "echod", "(D)D", echo));
    assert!(vm.register_native(k, "rb", "(ZI)Z", ret_r3));
    assert!(vm.register_native(k, "rbyte", "(ZI)B", ret_r3));
    assert!(vm.register_native(k, "rshort", "(ZI)S", ret_r3));
    assert!(vm.register_native(k, "rchar", "(ZI)C", ret_r3));
    assert!(vm.register_native(k, "rref", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", ret_r3));
    assert!(!vm.register_native(k, "bad", "oops", add));
    assert_eq!(vm.native_addr(k, "nope", "()V"), None);
    assert_eq!(vm.native_addr(k, "echo", "(J)J"), Some(echo));
    // Instance natives get the receiver in r1: inst(env, this, a) returns a.
    let inst = e.code(&[0xE1A0_0002, 0xE12F_FF1E]);
    assert!(vm.register_native(k, "inst", "(I)I", inst));
    let o = vm.alloc_object(k);
    let mid = vm.method_id(k, "inst", "(I)I", false).unwrap();
    assert_eq!(vm.invoke(&mut e.cpu, mid, Recv::Instance(o), &[Value::Int(31)], true), Ok(Value::Int(31)));
    // Host-side symbol for a not yet bound method.
    let s = vm.define("t/Sym").native("viaSymbol", "(II)I", true).id();
    assert_eq!(vm.native_addr(s, "viaSymbol", "(II)I"), None);

    let r = |e: &mut Env, name: &str, sig: &str, args: &[Value]| {
        let vm = e.vm.clone();
        let mid = vm.method_id(vm.class_named("t/N").unwrap(), name, sig, true).unwrap();
        vm.invoke(&mut e.cpu, mid, Recv::Static(vm.class_named("t/N").unwrap()), args, false)
    };
    assert_eq!(r(&mut e, "known", "(II)I", &[Value::Int(40), Value::Int(2)]), Ok(Value::Int(42)));
    assert_eq!(r(&mut e, "echo", "(J)J", &[Value::Long(-0x1234_5678_9abc)]), Ok(Value::Long(-0x1234_5678_9abc)));
    assert_eq!(r(&mut e, "echof", "(F)F", &[Value::Float(2.5)]), Ok(Value::Float(2.5)));
    assert_eq!(r(&mut e, "echod", "(D)D", &[Value::Double(-8.25)]), Ok(Value::Double(-8.25)));
    assert_eq!(r(&mut e, "rb", "(ZI)Z", &[Value::Int(1), Value::Int(0x100)]), Ok(Value::Int(0)));
    assert_eq!(r(&mut e, "rb", "(ZI)Z", &[Value::Int(1), Value::Int(1)]), Ok(Value::Int(1)));
    assert_eq!(r(&mut e, "rbyte", "(ZI)B", &[Value::Int(1), Value::Int(0xff)]), Ok(Value::Int(-1)));
    assert_eq!(r(&mut e, "rshort", "(ZI)S", &[Value::Int(1), Value::Int(0xffff)]), Ok(Value::Int(-1)));
    assert_eq!(r(&mut e, "rchar", "(ZI)C", &[Value::Int(1), Value::Int(0x1ffff)]), Ok(Value::Int(0xffff)));
    // A reference result is a handle the caller gets resolved: pass the second reference through.
    let o = vm.new_string("through");
    let got = r(
        &mut e,
        "rref",
        "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
        &[Value::Ref(None), Value::Ref(Some(o))],
    );
    assert_eq!(got, Ok(Value::Ref(Some(o))));
    assert_eq!(vm.live_refs(), 0, "locals of a native call are released");

    // Symbol resolver binds `Java_*` exports on first call.
    vm.set_symbol_resolver(move |n| (n == "Java_t_Sym_viaSymbol").then_some(add));
    assert_eq!(
        vm.call_static(&mut e.cpu, "t/Sym", "viaSymbol", "(II)I", &[Value::Int(1), Value::Int(2)]),
        Ok(Value::Int(3))
    );
    assert_eq!(vm.native_addr(s, "viaSymbol", "(II)I"), Some(add));
    // The overloaded long name is tried as well.
    vm.define("t/Sym").native("over", "(I)V", true);
    vm.set_symbol_resolver(move |n| (n == "Java_t_Sym_over__I").then_some(add));
    assert!(vm.call_static(&mut e.cpu, "t/Sym", "over", "(I)V", &[Value::Int(1)]).is_ok());
    // Without a binding a native is a reported stub.
    vm.define("t/Sym").native("unbound", "()I", true);
    vm.set_symbol_resolver(|_| None);
    assert_eq!(vm.call_static(&mut e.cpu, "t/Sym", "unbound", "()I", &[]), Ok(Value::Int(0)));
    let rep = vm.report();
    assert!(rep.stubs().any(|m| m.name == "unbound" && m.calls == 1));
    assert!(rep.members.iter().any(|m| m.name == "known" && m.kind == Kind::Native && m.calls == 1));
}

#[test]
fn native_failures_and_exceptions() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    let k = vm.define("t/X").id();
    // boom(env, cls, throwable): env->Throw(throwable)
    let boom = e.code(&[0xE92D_4000, 0xE1A0_1002, 0xE590_C000, 0xE59C_C034, 0xE12F_FF3C, 0xE8BD_8000]);
    // fault: write to unmapped memory
    let fault = e.code(&[0xE3A0_0000, 0xE580_0000, 0xE12F_FF1E]);
    assert!(vm.register_native(k, "boom", "(Ljava/lang/Throwable;)V", boom));
    assert!(vm.register_native(k, "fault", "()V", fault));
    let x = vm.new_exception("java/lang/IllegalStateException", "from native");
    let res = vm.call_static(&mut e.cpu, "t/X", "boom", "(Ljava/lang/Throwable;)V", &[Value::Ref(Some(x))]);
    assert_eq!(res, Err(Throw::Exc(x)));
    assert_eq!(vm.pending(e.cpu.processor_id()), None);
    let res = vm.call_static(&mut e.cpu, "t/X", "fault", "()V", &[]);
    assert!(matches!(res, Err(Throw::Stop(Stop::Fault(_)))), "{res:?}");
    assert_eq!(vm.live_refs(), 0);
}

#[test]
fn report_rendering() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    vm.define("t/R")
        .method("a", "()V", |_| Ok(Value::Void))
        .static_method("s", "()I", |_| Ok(Value::Int(1)))
        .field("f", "I");
    let k = vm.class_named("t/R").unwrap();
    vm.method_id(k, "a", "()V", false).unwrap();
    vm.method_id(k, "missing", "()V", false).unwrap();
    vm.jfield_id(k, "f", "I").unwrap();
    vm.find_class("t/Unknown").unwrap();
    vm.call_static(&mut e.cpu, "t/R", "s", "()I", &[]).unwrap();
    let rep = vm.report();
    let text = rep.render();
    assert!(text.contains("class-stub t/Unknown"));
    assert!(text.contains("host   t/R.a()V x0"));
    assert!(text.contains("host   static t/R.s()I x1"));
    assert!(text.contains("STUB   t/R.missing()V x0"));
    let nat = vm.define("t/R").native("nat", "()V", true).id();
    let ret = e.code(&[0xE12F_FF1E]);
    vm.register_native(nat, "nat", "()V", ret);
    assert!(vm.report().render().contains("native static t/R.nat()V x0"));
    assert!(text.contains("field  t/R.f I"));
    assert_eq!(rep.stubs().count(), 1);
}

#[test]
fn guest_memory_helpers() {
    let e = Env::new();
    let p = e.vm.galloc_bytes(b"abc", 1);
    assert_ne!(p, 0);
    assert_eq!(e.mem().read_bytes(p, 3).unwrap(), b"abc");
    e.vm.gfree(p);
    // A block that cannot be written (the allocator hands out an unmapped address).
    struct Bad;
    impl GuestAlloc for Bad {
        fn alloc(&mut self, _: u32) -> u32 {
            0xdead_0000
        }
        fn free(&mut self, _: u32) {}
    }
    let mem = e.mem();
    let vm = new_vm(mem, Box::new(Bad), VmConfig::new(Platform::default(), Box::new(NoAssets)));
    assert_eq!(vm.galloc_bytes(b"x", 0), 0);
    assert_eq!(vm.env_ptr(0), 0, "no tables before install");
    assert_eq!(vm.layout(), None);
    let _ = Imp::Stub;
}

#[test]
fn bare_vm_has_no_classes_but_object() {
    let mem = Arc::new(bhpc_mem::GuestMemory::new().unwrap());
    let heap = bhpc_mem::GuestHeap::new(mem.clone(), 0x4000_0000, 1 << 20).unwrap();
    let vm = Vm::bare(mem, Box::new(heap), VmConfig::new(Platform::default(), Box::new(NoAssets)));
    assert!(vm.class_named("java/lang/String").is_none());
    let s = vm.new_string("x");
    assert_eq!(vm.string(s).as_deref(), Some("x"));
    assert!(vm.now_ms() < 60_000);
}
