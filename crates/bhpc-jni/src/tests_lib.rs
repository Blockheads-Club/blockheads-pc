//! The built-in fake Java library, exercised through the Rust API.

use crate::tests::{test_platform, Env};
use crate::value::Ty;
use crate::*;
use bhpc_cpu::Stop;
use std::sync::{Arc, Mutex};

fn exc_class(e: &Env, r: JResult) -> String {
    match r {
        Err(Throw::Exc(x)) => e.vm.class_name(e.vm.class_of(x).unwrap()),
        other => panic!("expected an exception, got {other:?}"),
    }
}

fn int(v: JResult) -> i32 {
    v.unwrap().as_int()
}

fn text_of(e: &Env, v: JResult) -> String {
    e.s(obj(v))
}

macro_rules! text {
    ($e:expr, $v:expr) => {{
        let v = $v;
        text_of(&$e, v)
    }};
}

fn obj(v: JResult) -> Obj {
    v.unwrap().as_ref().expect("non-null result")
}

#[test]
fn object_class_and_loader() {
    let mut e = Env::new();
    let o = e.new_obj("java/lang/Object", "()V", &[]);
    let o2 = e.new_obj("java/lang/Object", "()V", &[]);
    assert_eq!(int(e.call(o, "hashCode", "()I", &[])), o.0 as i32);
    assert_eq!(e.call(o, "equals", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(o))]), Ok(Value::Int(1)));
    assert_eq!(e.call(o, "equals", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(o2))]), Ok(Value::Int(0)));
    let t = text!(e, e.call(o, "toString", "()Ljava/lang/String;", &[]));
    assert!(t.starts_with("java.lang.Object@"), "{t}");
    let c = obj(e.call(o, "getClass", "()Ljava/lang/Class;", &[]));
    assert_eq!(text!(e, e.call(c, "getName", "()Ljava/lang/String;", &[])), "java.lang.Object");
    assert_eq!(text!(e, e.call(c, "getSimpleName", "()Ljava/lang/String;", &[])), "Object");
    assert_eq!(text!(e, e.call(c, "toString", "()Ljava/lang/String;", &[])), "class java.lang.Object");
    assert_eq!(e.call(c, "getSuperclass", "()Ljava/lang/Class;", &[]), Ok(Value::Ref(None)));
    let sc = obj(e.scall(
        "java/lang/Class",
        "forName",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[e.string("java.lang.String")],
    ));
    let sup = obj(e.call(sc, "getSuperclass", "()Ljava/lang/Class;", &[]));
    assert_eq!(sup, c);
    assert_eq!(e.call(sc, "isInstance", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(o))]), Ok(Value::Int(0)));
    let s = e.vm.new_string("x");
    assert_eq!(e.call(sc, "isInstance", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(s))]), Ok(Value::Int(1)));
    assert_eq!(e.call(sc, "isInstance", "(Ljava/lang/Object;)Z", &[Value::Ref(None)]), Ok(Value::Int(0)));
    // Class.forName / ClassLoader.loadClass.
    let loader = obj(e.call(sc, "getClassLoader", "()Ljava/lang/ClassLoader;", &[]));
    let k = obj(e.call(loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[e.string("com.example.Foo")]));
    assert_eq!(e.vm.class_name(e.vm.class_of_mirror(k).unwrap()), "com/example/Foo");
    let k2 = obj(e.call(
        loader,
        "loadClass",
        "(Ljava/lang/String;Z)Ljava/lang/Class;",
        &[e.string("com.example.Foo"), Value::Int(1)],
    ));
    assert_eq!(k, k2);
    let k3 = obj(e.scall(
        "java/lang/Class",
        "forName",
        "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;",
        &[e.string("com.example.Foo"), Value::Int(1), Value::Ref(Some(loader))],
    ));
    assert_eq!(k, k3);
    let r = e.call(loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[Value::Ref(None)]);
    assert_eq!(exc_class(&e, r), "java/lang/NullPointerException");
    // Strict mode raises NoClassDefFoundError for unknown classes.
    let mut st = Env::with(|c| c.lenient = false);
    let r =
        st.scall("java/lang/Class", "forName", "(Ljava/lang/String;)Ljava/lang/Class;", &[st.string("nope.Missing")]);
    assert_eq!(exc_class(&st, r), "java/lang/NoClassDefFoundError");
}

#[test]
fn strings() {
    let mut e = Env::new();
    let s = e.vm.new_string("Hello, World");
    assert_eq!(int(e.call(s, "length", "()I", &[])), 12);
    assert_eq!(e.call(s, "isEmpty", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(int(e.call(s, "charAt", "(I)C", &[Value::Int(1)])), 'e' as i32);
    let r = e.call(s, "charAt", "(I)C", &[Value::Int(99)]);
    assert_eq!(exc_class(&e, r), "java/lang/StringIndexOutOfBoundsException");
    let r = e.call(s, "charAt", "(I)C", &[Value::Int(-1)]);
    assert_eq!(exc_class(&e, r), "java/lang/StringIndexOutOfBoundsException");
    assert_eq!(int(e.call(e.vm.new_string("ab"), "hashCode", "()I", &[])), 97 * 31 + 98);
    assert_eq!(e.call(s, "equals", "(Ljava/lang/Object;)Z", &[e.string("Hello, World")]), Ok(Value::Int(1)));
    assert_eq!(e.call(s, "equals", "(Ljava/lang/Object;)Z", &[e.string("x")]), Ok(Value::Int(0)));
    assert_eq!(e.call(s, "toString", "()Ljava/lang/String;", &[]), Ok(Value::Ref(Some(s))));
    let b = obj(e.call(s, "getBytes", "()[B", &[]));
    assert_eq!(e.vm.data(b), Some(Data::Prim(Ty::Byte, b"Hello, World".to_vec())));
    assert_eq!(text!(e, e.call(s, "substring", "(I)Ljava/lang/String;", &[Value::Int(7)])), "World");
    assert_eq!(text!(e, e.call(s, "substring", "(II)Ljava/lang/String;", &[Value::Int(0), Value::Int(5)])), "Hello");
    let r = e.call(s, "substring", "(II)Ljava/lang/String;", &[Value::Int(5), Value::Int(2)]);
    assert_eq!(exc_class(&e, r), "java/lang/StringIndexOutOfBoundsException");
    let r = e.call(s, "substring", "(I)Ljava/lang/String;", &[Value::Int(-1)]);
    assert_eq!(exc_class(&e, r), "java/lang/StringIndexOutOfBoundsException");
    assert_eq!(int(e.call(s, "indexOf", "(Ljava/lang/String;)I", &[e.string("World")])), 7);
    assert_eq!(int(e.call(s, "indexOf", "(Ljava/lang/String;)I", &[e.string("zz")])), -1);
    assert_eq!(int(e.call(s, "indexOf", "(Ljava/lang/String;)I", &[Value::Ref(None)])), 0);
    assert_eq!(e.call(s, "startsWith", "(Ljava/lang/String;)Z", &[e.string("Hell")]), Ok(Value::Int(1)));
    assert_eq!(e.call(s, "endsWith", "(Ljava/lang/String;)Z", &[e.string("rld")]), Ok(Value::Int(1)));
    assert_eq!(e.call(s, "contains", "(Ljava/lang/CharSequence;)Z", &[e.string("lo, W")]), Ok(Value::Int(1)));
    assert_eq!(e.call(s, "contains", "(Ljava/lang/CharSequence;)Z", &[Value::Ref(None)]), Ok(Value::Int(1)));
    assert_eq!(text!(e, e.call(s, "toLowerCase", "()Ljava/lang/String;", &[])), "hello, world");
    assert_eq!(text!(e, e.call(s, "toUpperCase", "()Ljava/lang/String;", &[])), "HELLO, WORLD");
    assert_eq!(text!(e, e.call(e.vm.new_string("  x "), "trim", "()Ljava/lang/String;", &[])), "x");
    assert_eq!(
        text!(e, e.call(s, "concat", "(Ljava/lang/String;)Ljava/lang/String;", &[e.string("!")])),
        "Hello, World!"
    );
    assert!(int(e.call(e.vm.new_string("a"), "compareTo", "(Ljava/lang/String;)I", &[e.string("b")])) < 0);
    // valueOf
    let o = e.vm.alloc_object(e.vm.class_named("java/lang/Object").unwrap());
    let names: Vec<(&str, &str, Value, &str)> = vec![
        ("(Ljava/lang/Object;)Ljava/lang/String;", "", Value::Ref(None), "null"),
        ("(Ljava/lang/Object;)Ljava/lang/String;", "", e.string("str"), "str"),
        ("(I)Ljava/lang/String;", "", Value::Int(-4), "-4"),
        ("(J)Ljava/lang/String;", "", Value::Long(1 << 40), "1099511627776"),
        ("(Z)Ljava/lang/String;", "", Value::Int(1), "true"),
    ];
    for (sig, _, arg, want) in names {
        assert_eq!(text!(e, e.scall("java/lang/String", "valueOf", sig, &[arg])), want);
    }
    let v = text!(
        e,
        e.scall("java/lang/String", "valueOf", "(Ljava/lang/Object;)Ljava/lang/String;", &[Value::Ref(Some(o))])
    );
    assert!(v.starts_with("java.lang.Object@"));
    // Constructors.
    let empty = e.new_obj("java/lang/String", "()V", &[]);
    assert_eq!(e.s(empty), "");
    let copy = e.new_obj("java/lang/String", "(Ljava/lang/String;)V", &[Value::Ref(Some(s))]);
    assert_eq!(e.s(copy), "Hello, World");
    let from_null = e.new_obj("java/lang/String", "(Ljava/lang/String;)V", &[Value::Ref(None)]);
    assert_eq!(e.s(from_null), "");
    let bytes = e.vm.new_byte_array("héllo".as_bytes());
    let from_bytes = e.new_obj("java/lang/String", "([B)V", &[Value::Ref(Some(bytes))]);
    assert_eq!(e.s(from_bytes), "héllo");
    let from_nothing = e.new_obj("java/lang/String", "([B)V", &[Value::Ref(None)]);
    assert_eq!(e.s(from_nothing), "");
}

#[test]
fn string_builders() {
    let mut e = Env::new();
    for class in ["java/lang/StringBuilder", "java/lang/StringBuffer"] {
        let b = e.new_obj(class, "()V", &[]);
        let sig = |p: &str| format!("({p})Ljava/lang/StringBuilder;");
        e.call(b, "append", &sig("Ljava/lang/String;"), &[e.string("a")]).unwrap();
        e.call(b, "append", &sig("Ljava/lang/String;"), &[Value::Ref(None)]).unwrap();
        e.call(b, "append", &sig("Ljava/lang/Object;"), &[e.string("b")]).unwrap();
        e.call(b, "append", &sig("I"), &[Value::Int(-1)]).unwrap();
        e.call(b, "append", &sig("J"), &[Value::Long(5_000_000_000)]).unwrap();
        e.call(b, "append", &sig("C"), &[Value::Int('z' as i32)]).unwrap();
        let r = e.call(b, "append", &sig("Z"), &[Value::Int(0)]).unwrap();
        assert_eq!(r, Value::Ref(Some(b)), "append returns this");
        assert_eq!(int(e.call(b, "length", "()I", &[])), "anullb-15000000000zfalse".len() as i32);
        assert_eq!(text!(e, e.call(b, "toString", "()Ljava/lang/String;", &[])), "anullb-15000000000zfalse");
    }
    let b = e.new_obj("java/lang/StringBuilder", "(Ljava/lang/String;)V", &[e.string("start")]);
    assert_eq!(text!(e, e.call(b, "toString", "()Ljava/lang/String;", &[])), "start");
    let b = e.new_obj("java/lang/StringBuilder", "(Ljava/lang/String;)V", &[Value::Ref(None)]);
    assert_eq!(text!(e, e.call(b, "toString", "()Ljava/lang/String;", &[])), "");
    e.new_obj("java/lang/StringBuilder", "(I)V", &[Value::Int(16)]);
}

#[test]
fn boxes() {
    let mut e = Env::new();
    let i = obj(e.scall("java/lang/Integer", "valueOf", "(I)Ljava/lang/Integer;", &[Value::Int(42)]));
    assert_eq!(int(e.call(i, "intValue", "()I", &[])), 42);
    assert_eq!(e.call(i, "longValue", "()J", &[]), Ok(Value::Long(42)));
    assert_eq!(e.call(i, "floatValue", "()F", &[]), Ok(Value::Float(42.0)));
    assert_eq!(e.call(i, "doubleValue", "()D", &[]), Ok(Value::Double(42.0)));
    assert_eq!(text!(e, e.call(i, "toString", "()Ljava/lang/String;", &[])), "42");
    assert_eq!(int(e.call(i, "hashCode", "()I", &[])), 42);
    let i2 = e.new_obj("java/lang/Integer", "(I)V", &[Value::Int(42)]);
    assert_eq!(e.call(i, "equals", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(i2))]), Ok(Value::Int(1)));
    let l = obj(e.scall("java/lang/Long", "valueOf", "(J)Ljava/lang/Long;", &[Value::Long(-(1 << 40))]));
    assert_eq!(int(e.call(l, "intValue", "()I", &[])), 0);
    assert_eq!(e.call(l, "longValue", "()J", &[]), Ok(Value::Long(-(1 << 40))));
    assert_eq!(e.call(l, "floatValue", "()F", &[]), Ok(Value::Float(-(1i64 << 40) as f32)));
    assert_eq!(e.call(l, "doubleValue", "()D", &[]), Ok(Value::Double(-(1i64 << 40) as f64)));
    assert_eq!(text!(e, e.call(l, "toString", "()Ljava/lang/String;", &[])), "-1099511627776");
    assert_eq!(int(e.call(l, "hashCode", "()I", &[])), ((-(1i64 << 40)) ^ ((-(1i64 << 40)) >> 32)) as i32);
    let f = obj(e.scall("java/lang/Float", "valueOf", "(F)Ljava/lang/Float;", &[Value::Float(2.5)]));
    assert_eq!(int(e.call(f, "intValue", "()I", &[])), 2);
    assert_eq!(e.call(f, "longValue", "()J", &[]), Ok(Value::Long(2)));
    assert_eq!(e.call(f, "floatValue", "()F", &[]), Ok(Value::Float(2.5)));
    assert_eq!(e.call(f, "doubleValue", "()D", &[]), Ok(Value::Double(2.5)));
    assert_eq!(int(e.call(f, "hashCode", "()I", &[])), 2.5f32.to_bits() as i32);
    assert_eq!(text!(e, e.call(f, "toString", "()Ljava/lang/String;", &[])), "2.5");
    let d = obj(e.scall("java/lang/Double", "valueOf", "(D)Ljava/lang/Double;", &[Value::Double(4.5)]));
    assert_eq!(int(e.call(d, "intValue", "()I", &[])), 4);
    assert_eq!(e.call(d, "longValue", "()J", &[]), Ok(Value::Long(4)));
    assert_eq!(e.call(d, "floatValue", "()F", &[]), Ok(Value::Float(4.5)));
    assert_eq!(e.call(d, "doubleValue", "()D", &[]), Ok(Value::Double(4.5)));
    assert_eq!(int(e.call(d, "hashCode", "()I", &[])), (4.5f64.to_bits() ^ (4.5f64.to_bits() >> 32)) as i32);
    assert_eq!(text!(e, e.call(d, "toString", "()Ljava/lang/String;", &[])), "4.5");
    assert_eq!(e.call(i, "floatValue", "()F", &[]), Ok(Value::Float(42.0)));
    let b = obj(e.scall("java/lang/Boolean", "valueOf", "(Z)Ljava/lang/Boolean;", &[Value::Int(1)]));
    assert_eq!(e.call(b, "booleanValue", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(text!(e, e.call(b, "toString", "()Ljava/lang/String;", &[])), "true");
    assert_eq!(int(e.call(b, "hashCode", "()I", &[])), 1);
    let c = obj(e.scall("java/lang/Character", "valueOf", "(C)Ljava/lang/Character;", &[Value::Int('q' as i32)]));
    assert_eq!(int(e.call(c, "charValue", "()C", &[])), 'q' as i32);
    assert_eq!(text!(e, e.call(c, "toString", "()Ljava/lang/String;", &[])), "q");
    let sh = obj(e.scall("java/lang/Short", "valueOf", "(S)Ljava/lang/Short;", &[Value::Int(-3)]));
    assert_eq!(text!(e, e.call(sh, "toString", "()Ljava/lang/String;", &[])), "-3");
    let by = obj(e.scall("java/lang/Byte", "valueOf", "(B)Ljava/lang/Byte;", &[Value::Int(-3)]));
    assert_eq!(int(e.call(by, "intValue", "()I", &[])), -3);
    // toString of an unboxed-but-box-named object and parseInt.
    assert_eq!(int(e.scall("java/lang/Integer", "parseInt", "(Ljava/lang/String;)I", &[e.string(" 17 ")])), 17);
    let r = e.scall("java/lang/Integer", "parseInt", "(Ljava/lang/String;)I", &[e.string("x")]);
    assert_eq!(exc_class(&e, r), "java/lang/NumberFormatException");
    let r = e.scall("java/lang/Integer", "parseInt", "(Ljava/lang/String;)I", &[Value::Ref(None)]);
    assert_eq!(exc_class(&e, r), "java/lang/NumberFormatException");
    // A box whose value was never set reads as 0 / false.
    let raw = e.vm.alloc_object(e.vm.class_named("java/lang/Integer").unwrap());
    assert_eq!(int(e.call(raw, "intValue", "()I", &[])), 0);
    assert_eq!(text!(e, e.call(raw, "toString", "()Ljava/lang/String;", &[])), "0");
    // A Number that is not a box has no value.
    let num = e.vm.alloc_object(e.vm.define("t/MyNumber").extends("java/lang/Number").id());
    assert_eq!(int(e.call(num, "intValue", "()I", &[])), 0);
    assert_eq!(e.call(num, "floatValue", "()F", &[]), Ok(Value::Float(0.0)));
}

#[test]
fn system_and_thread() {
    let mut e = Env::new();
    let t = int(e
        .scall("java/lang/System", "currentTimeMillis", "()J", &[])
        .map(|v| Value::Int((v.as_long() > 1_600_000_000_000) as i32)));
    assert_eq!(t, 1);
    e.clock.store(2000, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(e.scall("java/lang/System", "nanoTime", "()J", &[]), Ok(Value::Long(2_000_000_000)));
    let o = e.vm.new_string("x");
    assert_eq!(
        int(e.scall("java/lang/System", "identityHashCode", "(Ljava/lang/Object;)I", &[Value::Ref(Some(o))])),
        o.0 as i32
    );
    assert_eq!(int(e.scall("java/lang/System", "identityHashCode", "(Ljava/lang/Object;)I", &[Value::Ref(None)])), 0);
    e.scall("java/lang/System", "gc", "()V", &[]).unwrap();
    assert_eq!(text!(e, e.scall("java/lang/System", "lineSeparator", "()Ljava/lang/String;", &[])), "\n");
    std::env::set_var("BHPC_JNI_TEST_ENV", "yes");
    assert_eq!(
        text!(
            e,
            e.scall(
                "java/lang/System",
                "getenv",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[e.string("BHPC_JNI_TEST_ENV")]
            )
        ),
        "yes"
    );
    assert_eq!(
        e.scall("java/lang/System", "getenv", "(Ljava/lang/String;)Ljava/lang/String;", &[e.string("BHPC_NOPE_NOPE")]),
        Ok(Value::Ref(None))
    );
    assert_eq!(
        e.scall("java/lang/System", "getenv", "(Ljava/lang/String;)Ljava/lang/String;", &[Value::Ref(None)]),
        Ok(Value::Ref(None))
    );
    let prop = |e: &mut Env, k: &str| {
        e.scall("java/lang/System", "getProperty", "(Ljava/lang/String;)Ljava/lang/String;", &[e.string(k)])
    };
    for (k, v) in [
        ("os.name", "Linux"),
        ("os.arch", "armv7l"),
        ("java.vm.name", "Dalvik"),
        ("file.separator", "/"),
        ("path.separator", ":"),
        ("line.separator", "\n"),
        ("user.language", "tr"),
        ("user.region", "TR"),
        ("user.timezone", "Europe/Istanbul"),
    ] {
        assert_eq!(text!(e, prop(&mut e, k)), v, "{k}");
    }
    assert_eq!(text!(e, prop(&mut e, "user.dir")), test_platform().files_dir);
    assert_eq!(text!(e, prop(&mut e, "user.home")), test_platform().files_dir);
    assert_eq!(text!(e, prop(&mut e, "java.io.tmpdir")), Platform::default().cache_dir);
    assert_eq!(prop(&mut e, "no.such.property"), Ok(Value::Ref(None)));
    let with_default = e.scall(
        "java/lang/System",
        "getProperty",
        "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
        &[e.string("no.such"), e.string("dflt")],
    );
    assert_eq!(text!(e, with_default), "dflt");
    // arraycopy: primitives, objects, overlap, errors.
    let src = e.vm.new_byte_array(&[1, 2, 3, 4, 5]);
    let dst = e.vm.new_byte_array(&[0; 5]);
    let ac = |e: &mut Env, a: Obj, ap: i32, b: Obj, bp: i32, n: i32| {
        e.scall(
            "java/lang/System",
            "arraycopy",
            "(Ljava/lang/Object;ILjava/lang/Object;II)V",
            &[Value::Ref(Some(a)), Value::Int(ap), Value::Ref(Some(b)), Value::Int(bp), Value::Int(n)],
        )
    };
    ac(&mut e, src, 1, dst, 0, 3).unwrap();
    assert_eq!(e.vm.data(dst), Some(Data::Prim(Ty::Byte, vec![2, 3, 4, 0, 0])));
    let ints = e.vm.new_prim_array(Ty::Int, 5);
    let r = ac(&mut e, src, 0, ints, 0, 1);
    assert_eq!(exc_class(&e, r), "java/lang/ArrayStoreException");
    let r = ac(&mut e, src, 3, dst, 0, 3);
    assert_eq!(exc_class(&e, r), "java/lang/ArrayIndexOutOfBoundsException");
    let r = ac(&mut e, src, 0, dst, 4, 2);
    assert_eq!(exc_class(&e, r), "java/lang/ArrayIndexOutOfBoundsException");
    let r = ac(&mut e, src, -1, dst, 0, 1);
    assert_eq!(exc_class(&e, r), "java/lang/ArrayIndexOutOfBoundsException");
    let notarr = e.vm.new_string("s");
    let r = ac(&mut e, notarr, 0, dst, 0, 0);
    assert_eq!(exc_class(&e, r), "java/lang/ArrayStoreException");
    let r = e.scall(
        "java/lang/System",
        "arraycopy",
        "(Ljava/lang/Object;ILjava/lang/Object;II)V",
        &[Value::Ref(None), Value::Int(0), Value::Ref(Some(dst)), Value::Int(0), Value::Int(0)],
    );
    assert_eq!(exc_class(&e, r), "java/lang/NullPointerException");
    let a1 = e.vm.new_object_array("java/lang/Object", 3);
    let a2 = e.vm.new_object_array("java/lang/Object", 3);
    let s = e.vm.new_string("v");
    e.vm.with_data(a1, |d| *d = Data::Objs(vec![Some(s), None, Some(s)]));
    ac(&mut e, a1, 0, a2, 1, 2).unwrap();
    assert_eq!(e.vm.data(a2), Some(Data::Objs(vec![None, Some(s), None])));
    // exit stops the CPU.
    e.scall("java/lang/System", "exit", "(I)V", &[Value::Int(3)]).unwrap();
    // Thread.
    let cur = obj(e.scall("java/lang/Thread", "currentThread", "()Ljava/lang/Thread;", &[]));
    assert_eq!(text!(e, e.call(cur, "getName", "()Ljava/lang/String;", &[])), "main");
    assert_eq!(e.call(cur, "getId", "()J", &[]), Ok(Value::Long(1)));
    assert_eq!(obj(e.scall("java/lang/Thread", "currentThread", "()Ljava/lang/Thread;", &[])), cur);
    e.call(cur, "setName", "(Ljava/lang/String;)V", &[e.string("renamed")]).unwrap();
    assert_eq!(text!(e, e.call(cur, "getName", "()Ljava/lang/String;", &[])), "renamed");
    assert_eq!(int(e.call(cur, "getPriority", "()I", &[])), 5);
    e.call(cur, "setPriority", "(I)V", &[Value::Int(1)]).unwrap();
    assert_eq!(e.call(cur, "isInterrupted", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.call(cur, "isAlive", "()Z", &[]), Ok(Value::Int(1)));
    assert!(e.call(cur, "getContextClassLoader", "()Ljava/lang/ClassLoader;", &[]).is_ok());
    let fresh = e.new_obj("java/lang/Thread", "()V", &[]);
    assert_eq!(text!(e, e.call(fresh, "getName", "()Ljava/lang/String;", &[])), "Thread");
    e.scall("java/lang/Thread", "sleep", "(J)V", &[Value::Long(1)]).unwrap();
    e.scall("java/lang/Thread", "sleep", "(J)V", &[Value::Long(-5)]).unwrap();
    let other = crate::jlib::thread_object(&e.vm, 7);
    assert_eq!(text!(e, e.call(other, "getName", "()Ljava/lang/String;", &[])), "Thread-7");
}

#[test]
fn system_exit_requests_a_stop() {
    let mut e = Env::new();
    // Reach System.exit through the JNI tables from guest code, so the stop is observed by `run`.
    let ch = e.class_h("java/lang/System");
    let n = e.cstr("exit");
    let s = e.cstr("(I)V");
    let mid = e.jni("GetStaticMethodID", &[ch, n, s]).r0;
    let b = e.buf(8);
    e.mem().write_u32(b, 9).unwrap();
    let r = e.try_jni("CallStaticVoidMethodA", &[ch, mid, b]);
    assert_eq!(r, Err(Stop::Host("System.exit(9)".into())));
}

#[test]
fn throwables() {
    let mut e = Env::new();
    let x = e.new_obj("java/lang/IllegalStateException", "(Ljava/lang/String;)V", &[e.string("bad")]);
    assert_eq!(text!(e, e.call(x, "getMessage", "()Ljava/lang/String;", &[])), "bad");
    assert_eq!(text!(e, e.call(x, "getLocalizedMessage", "()Ljava/lang/String;", &[])), "bad");
    assert_eq!(e.call(x, "getCause", "()Ljava/lang/Throwable;", &[]), Ok(Value::Ref(None)));
    assert_eq!(text!(e, e.call(x, "toString", "()Ljava/lang/String;", &[])), "java.lang.IllegalStateException: bad");
    e.call(x, "printStackTrace", "()V", &[]).unwrap();
    assert!(e.log_text().contains("java.lang.IllegalStateException: bad"));
    let y = e.new_obj("java/lang/RuntimeException", "()V", &[]);
    assert_eq!(text!(e, e.call(y, "toString", "()Ljava/lang/String;", &[])), "java.lang.RuntimeException");
    assert!(e.vm.instance_of(x, "java/lang/RuntimeException"));
    let ex = e.vm.new_exception("java/lang/ArrayIndexOutOfBoundsException", "i");
    assert!(e.vm.instance_of(ex, "java/lang/IndexOutOfBoundsException"));
    let ex = e.vm.new_exception("java/io/FileNotFoundException", "f");
    assert!(e.vm.instance_of(ex, "java/io/IOException"));
}

#[test]
fn collections() {
    let mut e = Env::new();
    for class in [
        "java/util/ArrayList",
        "java/util/LinkedList",
        "java/util/Vector",
        "java/util/LinkedHashSet",
        "java/util/HashSet",
    ] {
        let l = e.new_obj(class, "()V", &[]);
        assert_eq!(e.call(l, "isEmpty", "()Z", &[]), Ok(Value::Int(1)));
        let (a, b, c) = (e.vm.new_string("a"), e.vm.new_string("b"), e.vm.new_string("c"));
        assert_eq!(e.call(l, "add", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(a))]), Ok(Value::Int(1)));
        e.call(l, "add", "(Ljava/lang/Object;)Z", &[Value::Ref(Some(c))]).unwrap();
        e.call(l, "add", "(ILjava/lang/Object;)V", &[Value::Int(1), Value::Ref(Some(b))]).unwrap();
        assert_eq!(int(e.call(l, "size", "()I", &[])), 3);
        assert_eq!(e.call(l, "get", "(I)Ljava/lang/Object;", &[Value::Int(1)]), Ok(Value::Ref(Some(b))));
        let old = e.call(l, "set", "(ILjava/lang/Object;)Ljava/lang/Object;", &[Value::Int(1), Value::Ref(Some(a))]);
        assert_eq!(old, Ok(Value::Ref(Some(b))));
        assert_eq!(int(e.call(l, "indexOf", "(Ljava/lang/Object;)I", &[e.string("c")])), 2);
        assert_eq!(int(e.call(l, "indexOf", "(Ljava/lang/Object;)I", &[e.string("zz")])), -1);
        assert_eq!(e.call(l, "contains", "(Ljava/lang/Object;)Z", &[e.string("a")]), Ok(Value::Int(1)));
        assert_eq!(e.call(l, "contains", "(Ljava/lang/Object;)Z", &[e.string("zz")]), Ok(Value::Int(0)));
        assert_eq!(e.call(l, "remove", "(I)Ljava/lang/Object;", &[Value::Int(0)]), Ok(Value::Ref(Some(a))));
        // iterator drains a copy
        let it = obj(e.call(l, "iterator", "()Ljava/util/Iterator;", &[]));
        assert_eq!(e.call(it, "hasNext", "()Z", &[]), Ok(Value::Int(1)));
        assert_eq!(e.call(it, "next", "()Ljava/lang/Object;", &[]), Ok(Value::Ref(Some(a))));
        assert_eq!(e.call(it, "next", "()Ljava/lang/Object;", &[]), Ok(Value::Ref(Some(c))));
        assert_eq!(e.call(it, "hasNext", "()Z", &[]), Ok(Value::Int(0)));
        let r = e.call(it, "next", "()Ljava/lang/Object;", &[]);
        assert_eq!(exc_class(&e, r), "java/util/NoSuchElementException");
        assert_eq!(int(e.call(l, "size", "()I", &[])), 2, "the list itself is untouched by iteration");
        let arr = obj(e.call(l, "toArray", "()[Ljava/lang/Object;", &[]));
        assert_eq!(e.vm.array_len(arr), Some(2));
        // out of range
        for (name, sig, args) in [
            ("get", "(I)Ljava/lang/Object;", vec![Value::Int(5)]),
            ("get", "(I)Ljava/lang/Object;", vec![Value::Int(-1)]),
            ("set", "(ILjava/lang/Object;)Ljava/lang/Object;", vec![Value::Int(5), Value::Ref(None)]),
            ("remove", "(I)Ljava/lang/Object;", vec![Value::Int(5)]),
            ("add", "(ILjava/lang/Object;)V", vec![Value::Int(9), Value::Ref(None)]),
        ] {
            let r = e.call(l, name, sig, &args);
            assert_eq!(exc_class(&e, r), "java/lang/IndexOutOfBoundsException", "{name}");
        }
        e.call(l, "clear", "()V", &[]).unwrap();
        assert_eq!(int(e.call(l, "size", "()I", &[])), 0);
    }
    let l = e.new_obj("java/util/ArrayList", "(I)V", &[Value::Int(4)]);
    let other = e.new_obj("java/util/ArrayList", "()V", &[]);
    e.call(other, "add", "(Ljava/lang/Object;)Z", &[e.string("x")]).unwrap();
    assert_eq!(e.call(l, "addAll", "(Ljava/util/Collection;)Z", &[Value::Ref(Some(other))]), Ok(Value::Int(1)));
    assert_eq!(e.call(l, "addAll", "(Ljava/util/Collection;)Z", &[Value::Ref(None)]), Ok(Value::Int(0)));
    assert_eq!(int(e.call(l, "size", "()I", &[])), 1);
}

#[test]
fn maps() {
    let mut e = Env::new();
    for class in ["java/util/HashMap", "java/util/LinkedHashMap", "java/util/TreeMap"] {
        let m = e.new_obj(class, "()V", &[]);
        assert_eq!(e.call(m, "isEmpty", "()Z", &[]), Ok(Value::Int(1)));
        let put = "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;";
        let v1 = e.vm.new_string("v1");
        let v2 = e.vm.new_string("v2");
        assert_eq!(e.call(m, "put", put, &[e.string("k"), Value::Ref(Some(v1))]), Ok(Value::Ref(None)));
        assert_eq!(e.call(m, "put", put, &[e.string("k"), Value::Ref(Some(v2))]), Ok(Value::Ref(Some(v1))));
        e.call(m, "put", put, &[e.string("j"), Value::Ref(Some(v1))]).unwrap();
        assert_eq!(int(e.call(m, "size", "()I", &[])), 2);
        assert_eq!(
            e.call(m, "get", "(Ljava/lang/Object;)Ljava/lang/Object;", &[e.string("k")]),
            Ok(Value::Ref(Some(v2)))
        );
        assert_eq!(e.call(m, "get", "(Ljava/lang/Object;)Ljava/lang/Object;", &[e.string("zz")]), Ok(Value::Ref(None)));
        assert_eq!(e.call(m, "containsKey", "(Ljava/lang/Object;)Z", &[e.string("j")]), Ok(Value::Int(1)));
        assert_eq!(e.call(m, "containsKey", "(Ljava/lang/Object;)Z", &[e.string("zz")]), Ok(Value::Int(0)));
        let keys = obj(e.call(m, "keySet", "()Ljava/util/Set;", &[]));
        assert_eq!(int(e.call(keys, "size", "()I", &[])), 2);
        let vals = obj(e.call(m, "values", "()Ljava/util/Collection;", &[]));
        assert_eq!(int(e.call(vals, "size", "()I", &[])), 2);
        assert_eq!(
            e.call(m, "remove", "(Ljava/lang/Object;)Ljava/lang/Object;", &[e.string("j")]),
            Ok(Value::Ref(Some(v1)))
        );
        assert_eq!(
            e.call(m, "remove", "(Ljava/lang/Object;)Ljava/lang/Object;", &[e.string("j")]),
            Ok(Value::Ref(None))
        );
        e.call(m, "clear", "()V", &[]).unwrap();
        assert_eq!(int(e.call(m, "size", "()I", &[])), 0);
        // null keys are allowed
        e.call(m, "put", put, &[Value::Ref(None), Value::Ref(Some(v1))]).unwrap();
        assert_eq!(
            e.call(m, "get", "(Ljava/lang/Object;)Ljava/lang/Object;", &[Value::Ref(None)]),
            Ok(Value::Ref(Some(v1)))
        );
    }
    e.new_obj("java/util/HashMap", "(I)V", &[Value::Int(8)]);
}

#[test]
fn locale_timezone_uuid() {
    let mut e = Env::new();
    let l = obj(e.scall("java/util/Locale", "getDefault", "()Ljava/util/Locale;", &[]));
    assert_eq!(text!(e, e.call(l, "getLanguage", "()Ljava/lang/String;", &[])), "tr");
    assert_eq!(text!(e, e.call(l, "getCountry", "()Ljava/lang/String;", &[])), "TR");
    assert_eq!(text!(e, e.call(l, "toString", "()Ljava/lang/String;", &[])), "tr_TR");
    let tz = obj(e.scall("java/util/TimeZone", "getDefault", "()Ljava/util/TimeZone;", &[]));
    assert_eq!(text!(e, e.call(tz, "getID", "()Ljava/lang/String;", &[])), "Europe/Istanbul");
    assert_eq!(int(e.call(tz, "getRawOffset", "()I", &[])), 0);
    let u1 = obj(e.scall("java/util/UUID", "randomUUID", "()Ljava/util/UUID;", &[]));
    let u2 = obj(e.scall("java/util/UUID", "randomUUID", "()Ljava/util/UUID;", &[]));
    let (s1, s2) = (
        text!(e, e.call(u1, "toString", "()Ljava/lang/String;", &[])),
        text!(e, e.call(u2, "toString", "()Ljava/lang/String;", &[])),
    );
    assert_ne!(s1, s2);
    for s in [s1, s2] {
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12], "{s}");
        assert!(parts[2].starts_with('4'));
        assert!("89ab".contains(&parts[3][..1]));
    }
    // A locale without a country.
    let mut p = Env::with(|c| c.platform.locale = "de".into());
    let l = obj(p.scall("java/util/Locale", "getDefault", "()Ljava/util/Locale;", &[]));
    assert_eq!(text!(p, p.call(l, "toString", "()Ljava/lang/String;", &[])), "de");
}

#[test]
fn files_and_streams() {
    let mut e = Env::new();
    let dir = std::env::temp_dir().join(format!("bhpc-jni-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let d = dir.to_string_lossy().into_owned();
    let f = e.new_obj("java/io/File", "(Ljava/lang/String;)V", &[e.string(&d)]);
    assert_eq!(e.call(f, "exists", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.call(f, "mkdirs", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(e.call(f, "isDirectory", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(e.call(f, "isFile", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.call(f, "canRead", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(e.call(f, "canWrite", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(e.call(f, "mkdir", "()Z", &[]), Ok(Value::Int(0)), "already exists");
    assert_eq!(text!(e, e.call(f, "getPath", "()Ljava/lang/String;", &[])), d);
    assert_eq!(text!(e, e.call(f, "toString", "()Ljava/lang/String;", &[])), d);
    assert_eq!(text!(e, e.call(f, "getAbsolutePath", "()Ljava/lang/String;", &[])), d);
    // (File, String) and (String, String) constructors.
    let child =
        e.new_obj("java/io/File", "(Ljava/io/File;Ljava/lang/String;)V", &[Value::Ref(Some(f)), e.string("data.bin")]);
    let child2 = e.new_obj(
        "java/io/File",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[e.string(&format!("{d}/")), e.string("data.bin")],
    );
    let empty_dir =
        e.new_obj("java/io/File", "(Ljava/lang/String;Ljava/lang/String;)V", &[e.string(""), e.string("rel")]);
    assert_eq!(text!(e, e.call(empty_dir, "getPath", "()Ljava/lang/String;", &[])), "rel");
    let null_dir =
        e.new_obj("java/io/File", "(Ljava/io/File;Ljava/lang/String;)V", &[Value::Ref(None), e.string("rel2")]);
    assert_eq!(text!(e, e.call(null_dir, "getPath", "()Ljava/lang/String;", &[])), "rel2");
    let cp = text!(e, e.call(child, "getPath", "()Ljava/lang/String;", &[]));
    assert_eq!(cp, format!("{d}/data.bin"));
    assert_eq!(text!(e, e.call(child2, "getPath", "()Ljava/lang/String;", &[])), cp);
    assert_eq!(text!(e, e.call(child, "getName", "()Ljava/lang/String;", &[])), "data.bin");
    assert_eq!(text!(e, e.call(child, "getParent", "()Ljava/lang/String;", &[])), d);
    let pf = obj(e.call(child, "getParentFile", "()Ljava/io/File;", &[]));
    assert_eq!(text!(e, e.call(pf, "getPath", "()Ljava/lang/String;", &[])), d);
    let bare = e.new_obj("java/io/File", "(Ljava/lang/String;)V", &[e.string("bare")]);
    assert_eq!(e.call(bare, "getParent", "()Ljava/lang/String;", &[]), Ok(Value::Ref(None)));
    assert_eq!(e.call(bare, "getParentFile", "()Ljava/io/File;", &[]), Ok(Value::Ref(None)));
    assert_eq!(e.call(child, "exists", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.call(child, "length", "()J", &[]), Ok(Value::Long(0)));
    std::fs::write(dir.join("data.bin"), [10u8, 20, 30, 40, 50]).unwrap();
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    assert_eq!(e.call(child, "length", "()J", &[]), Ok(Value::Long(5)));
    assert_eq!(e.call(child, "isFile", "()Z", &[]), Ok(Value::Int(1)));
    let names = obj(e.call(f, "list", "()[Ljava/lang/String;", &[]));
    let Some(Data::Objs(items)) = e.vm.data(names) else { panic!() };
    assert_eq!(items.iter().map(|o| e.s(o.unwrap())).collect::<Vec<_>>(), ["a.txt", "data.bin"]);
    assert_eq!(e.call(child, "list", "()[Ljava/lang/String;", &[]), Ok(Value::Ref(None)));
    // FileInputStream reads the file.
    let fis = e.new_obj("java/io/FileInputStream", "(Ljava/io/File;)V", &[Value::Ref(Some(child))]);
    assert_eq!(int(e.call(fis, "available", "()I", &[])), 5);
    assert_eq!(int(e.call(fis, "read", "()I", &[])), 10);
    let buf = e.vm.new_prim_array(Ty::Byte, 8);
    assert_eq!(int(e.call(fis, "read", "([B)I", &[Value::Ref(Some(buf))])), 4);
    assert_eq!(e.vm.data(buf), Some(Data::Prim(Ty::Byte, vec![20, 30, 40, 50, 0, 0, 0, 0])));
    assert_eq!(int(e.call(fis, "read", "()I", &[])), -1);
    assert_eq!(int(e.call(fis, "read", "([B)I", &[Value::Ref(Some(buf))])), -1);
    e.call(fis, "close", "()V", &[]).unwrap();
    let fis2 = e.new_obj("java/io/FileInputStream", "(Ljava/lang/String;)V", &[e.string(&cp)]);
    assert_eq!(e.call(fis2, "skip", "(J)J", &[Value::Long(100)]), Ok(Value::Long(5)));
    assert_eq!(e.call(fis2, "skip", "(J)J", &[Value::Long(-3)]), Ok(Value::Long(0)));
    let mp = e.string(&format!("{d}/missing"));
    let missing = e.vm.clone().new_object(&mut e.cpu, "java/io/FileInputStream", "(Ljava/lang/String;)V", &[mp]);
    assert!(matches!(missing, Err(Throw::Exc(x)) if e.vm.instance_of(x, "java/io/FileNotFoundException")));
    assert_eq!(e.call(child, "delete", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(e.call(child, "delete", "()Z", &[]), Ok(Value::Int(0)));
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    assert_eq!(e.call(f, "delete", "()Z", &[]), Ok(Value::Int(1)), "empty directories can be deleted");
    // ByteArrayInputStream argument handling.
    let src = e.vm.new_byte_array(&[1, 2, 3]);
    let bis = e.new_obj("java/io/ByteArrayInputStream", "([B)V", &[Value::Ref(Some(src))]);
    let part = e.vm.new_prim_array(Ty::Byte, 4);
    assert_eq!(int(e.call(bis, "read", "([BII)I", &[Value::Ref(Some(part)), Value::Int(1), Value::Int(2)])), 2);
    assert_eq!(int(e.call(bis, "read", "([BII)I", &[Value::Ref(Some(part)), Value::Int(0), Value::Int(0)])), 0);
    for (off, len) in [(-1, 1), (0, -1), (3, 2)] {
        let r = e.call(bis, "read", "([BII)I", &[Value::Ref(Some(part)), Value::Int(off), Value::Int(len)]);
        assert_eq!(exc_class(&e, r), "java/lang/IndexOutOfBoundsException");
    }
    let r = e.call(bis, "read", "([BII)I", &[Value::Ref(None), Value::Int(0), Value::Int(1)]);
    assert_eq!(exc_class(&e, r), "java/lang/NullPointerException");
    let r = e.call(bis, "read", "([B)I", &[Value::Ref(None)]);
    assert_eq!(exc_class(&e, r), "java/lang/NullPointerException");
    let none = e.new_obj("java/io/ByteArrayInputStream", "([B)V", &[Value::Ref(None)]);
    assert_eq!(int(e.call(none, "available", "()I", &[])), 0);
    // Stream methods on an object that is not a stream degrade gracefully.
    let plain = e.vm.alloc_object(e.vm.class_named("java/io/ByteArrayInputStream").unwrap());
    e.vm.with_data(plain, |d| *d = Data::None);
    assert_eq!(int(e.call(plain, "available", "()I", &[])), 0);
    assert_eq!(int(e.call(plain, "read", "()I", &[])), -1);
}

#[test]
fn context_and_directories() {
    let mut e = Env::new();
    let app = e.activity();
    let p = test_platform();
    let path = |e: &mut Env, name: &str, sig: &str, args: &[Value]| -> String {
        let f = obj(e.call(app, name, sig, args));
        text!(e, e.call(f, "getPath", "()Ljava/lang/String;", &[]))
    };
    assert_eq!(path(&mut e, "getFilesDir", "()Ljava/io/File;", &[]), p.files_dir);
    assert_eq!(path(&mut e, "getCacheDir", "()Ljava/io/File;", &[]), p.cache_dir);
    assert_eq!(path(&mut e, "getObbDir", "()Ljava/io/File;", &[]), p.obb_dir);
    assert_eq!(path(&mut e, "getExternalCacheDir", "()Ljava/io/File;", &[]), format!("{}/cache", p.external_dir));
    assert_eq!(
        path(&mut e, "getExternalFilesDir", "(Ljava/lang/String;)Ljava/io/File;", &[Value::Ref(None)]),
        format!("{}/files", p.external_dir)
    );
    let saves = e.string("saves");
    assert_eq!(
        path(&mut e, "getExternalFilesDir", "(Ljava/lang/String;)Ljava/io/File;", &[saves]),
        format!("{}/files/saves", p.external_dir)
    );
    let xdat = e.string("x.dat");
    assert_eq!(
        path(&mut e, "getFileStreamPath", "(Ljava/lang/String;)Ljava/io/File;", &[xdat]),
        format!("{}/x.dat", p.files_dir)
    );
    let ddb = e.string("d.db");
    assert_eq!(
        path(&mut e, "getDatabasePath", "(Ljava/lang/String;)Ljava/io/File;", &[ddb]),
        format!("{}/databases/d.db", p.files_dir)
    );
    let sub = e.string("sub");
    let dir = path(&mut e, "getDir", "(Ljava/lang/String;I)Ljava/io/File;", &[sub, Value::Int(0)]);
    assert_eq!(dir, format!("{}/app_sub", p.files_dir));
    assert!(std::path::Path::new(&dir).is_dir());
    let _ = std::fs::remove_dir_all(&p.files_dir);
    assert_eq!(text!(e, e.call(app, "getPackageName", "()Ljava/lang/String;", &[])), "com.majicjungle.blockheads");
    assert_eq!(text!(e, e.call(app, "getPackageCodePath", "()Ljava/lang/String;", &[])), "/tmp/test.apk");
    assert_eq!(text!(e, e.call(app, "getPackageResourcePath", "()Ljava/lang/String;", &[])), "/tmp/test.apk");
    let ml = obj(e.call(app, "getMainLooper", "()Landroid/os/Looper;", &[]));
    assert_eq!(ml, e.vm.main_looper());
    let ctx = obj(e.call(app, "getApplicationContext", "()Landroid/content/Context;", &[]));
    assert_eq!(e.vm.class_name(e.vm.class_of(ctx).unwrap()), "com/apportable/app/VerdeApplication");
    assert!(e.vm.instance_of(ctx, "android/content/Context"));
    let app2 = obj(e.call(app, "getApplication", "()Landroid/app/Application;", &[]));
    assert_eq!(app2, ctx);
    assert!(e.call(app, "getClassLoader", "()Ljava/lang/ClassLoader;", &[]).unwrap().as_ref().is_some());
    let info = obj(e.call(app, "getApplicationInfo", "()Landroid/content/pm/ApplicationInfo;", &[]));
    assert_eq!(e.vm.string(e.vm.get_field(info, "sourceDir").as_ref().unwrap()).as_deref(), Some("/tmp/test.apk"));
    assert_eq!(
        e.vm.string(e.vm.get_field(info, "packageName").as_ref().unwrap()).as_deref(),
        Some("com.majicjungle.blockheads")
    );
    // System services.
    for (name, class) in [
        ("window", "android/view/WindowManager"),
        ("audio", "android/media/AudioManager"),
        ("connectivity", "android/net/ConnectivityManager"),
        ("input_method", "android/view/inputmethod/InputMethodManager"),
        ("phone", "android/telephony/TelephonyManager"),
        ("sensor", "android/hardware/SensorManager"),
        ("vibrator", "android/os/Vibrator"),
        ("clipboard", "android/content/ClipboardManager"),
        ("notification", "android/app/NotificationManager"),
        ("power", "android/os/PowerManager"),
        ("wifi", "android/net/wifi/WifiManager"),
        ("location", "android/location/LocationManager"),
        ("layout_inflater", "android/view/LayoutInflater"),
        ("activity", "android/app/ActivityManager"),
        ("alarm", "android/app/AlarmManager"),
    ] {
        let s = obj(e.call(app, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[e.string(name)]));
        assert_eq!(e.vm.class_name(e.vm.class_of(s).unwrap()), class);
        let s2 = obj(e.call(app, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[e.string(name)]));
        assert_eq!(s, s2);
    }
    assert_eq!(
        e.call(app, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[e.string("bogus")]),
        Ok(Value::Ref(None))
    );
    assert_eq!(
        e.call(app, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[Value::Ref(None)]),
        Ok(Value::Ref(None))
    );
    // Audio and connectivity.
    let am = obj(e.call(app, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[e.string("audio")]));
    assert_eq!(int(e.call(am, "getStreamVolume", "(I)I", &[Value::Int(3)])), 8);
    assert_eq!(int(e.call(am, "getStreamMaxVolume", "(I)I", &[Value::Int(3)])), 15);
    assert_eq!(
        int(e.call(
            am,
            "requestAudioFocus",
            "(Landroid/media/AudioManager$OnAudioFocusChangeListener;II)I",
            &[Value::Ref(None), Value::Int(3), Value::Int(1)]
        )),
        1
    );
    assert_eq!(
        int(e.call(
            am,
            "abandonAudioFocus",
            "(Landroid/media/AudioManager$OnAudioFocusChangeListener;)I",
            &[Value::Ref(None)]
        )),
        1
    );
    let prop =
        |e: &mut Env, k: &str| e.call(am, "getProperty", "(Ljava/lang/String;)Ljava/lang/String;", &[e.string(k)]);
    assert_eq!(text!(e, prop(&mut e, "android.media.property.OUTPUT_SAMPLE_RATE")), "44100");
    assert_eq!(text!(e, prop(&mut e, "android.media.property.OUTPUT_FRAMES_PER_BUFFER")), "256");
    assert_eq!(prop(&mut e, "x"), Ok(Value::Ref(None)));
    assert_eq!(e.vm.get_static(e.vm.class_named("android/media/AudioManager").unwrap(), "STREAM_MUSIC"), Value::Int(3));
    let cm =
        obj(e.call(app, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[e.string("connectivity")]));
    let ni = obj(e.call(cm, "getActiveNetworkInfo", "()Landroid/net/NetworkInfo;", &[]));
    for m in ["isConnected", "isAvailable", "isConnectedOrConnecting"] {
        assert_eq!(e.call(ni, m, "()Z", &[]), Ok(Value::Int(1)), "{m}");
    }
    assert_eq!(int(e.call(ni, "getType", "()I", &[])), 1);
    assert_eq!(text!(e, e.call(ni, "getTypeName", "()Ljava/lang/String;", &[])), "WIFI");
}

#[test]
fn display_resources_and_assets() {
    let mut e = Env::new();
    let app = e.activity();
    let wm = obj(e.call(app, "getWindowManager", "()Landroid/view/WindowManager;", &[]));
    let disp = obj(e.call(wm, "getDefaultDisplay", "()Landroid/view/Display;", &[]));
    assert_eq!(int(e.call(disp, "getWidth", "()I", &[])), 800);
    assert_eq!(int(e.call(disp, "getHeight", "()I", &[])), 480);
    assert_eq!(int(e.call(disp, "getRotation", "()I", &[])), 0);
    assert_eq!(int(e.call(disp, "getOrientation", "()I", &[])), 0);
    assert_eq!(int(e.call(disp, "getDisplayId", "()I", &[])), 0);
    assert_eq!(e.call(disp, "getRefreshRate", "()F", &[]), Ok(Value::Float(60.0)));
    for m in ["getMetrics", "getRealMetrics"] {
        let dm = e.new_obj("android/util/DisplayMetrics", "()V", &[]);
        e.call(disp, m, "(Landroid/util/DisplayMetrics;)V", &[Value::Ref(Some(dm))]).unwrap();
        assert_eq!(e.vm.get_field(dm, "widthPixels"), Value::Int(800));
        assert_eq!(e.vm.get_field(dm, "heightPixels"), Value::Int(480));
        assert_eq!(e.vm.get_field(dm, "density"), Value::Float(1.5));
        assert_eq!(e.vm.get_field(dm, "scaledDensity"), Value::Float(1.5));
        assert_eq!(e.vm.get_field(dm, "densityDpi"), Value::Int(240));
        assert_eq!(e.vm.get_field(dm, "xdpi"), Value::Float(240.0));
        let r = e.call(disp, m, "(Landroid/util/DisplayMetrics;)V", &[Value::Ref(None)]);
        assert_eq!(exc_class(&e, r), "java/lang/NullPointerException");
    }
    for m in ["getSize", "getRealSize"] {
        let pt = e.new_obj("android/graphics/Point", "()V", &[]);
        e.call(disp, m, "(Landroid/graphics/Point;)V", &[Value::Ref(Some(pt))]).unwrap();
        assert_eq!((e.vm.get_field(pt, "x"), e.vm.get_field(pt, "y")), (Value::Int(800), Value::Int(480)));
        let r = e.call(disp, m, "(Landroid/graphics/Point;)V", &[Value::Ref(None)]);
        assert_eq!(exc_class(&e, r), "java/lang/NullPointerException");
    }
    // Resources.
    let res = obj(e.call(app, "getResources", "()Landroid/content/res/Resources;", &[]));
    let dm = obj(e.call(res, "getDisplayMetrics", "()Landroid/util/DisplayMetrics;", &[]));
    assert_eq!(e.vm.get_field(dm, "widthPixels"), Value::Int(800));
    let cfg = obj(e.call(res, "getConfiguration", "()Landroid/content/res/Configuration;", &[]));
    assert_eq!(e.vm.get_field(cfg, "orientation"), Value::Int(2), "landscape");
    assert_eq!(e.vm.get_field(cfg, "screenWidthDp"), Value::Int(533));
    assert_eq!(e.vm.get_field(cfg, "screenHeightDp"), Value::Int(320));
    assert_eq!(e.vm.get_field(cfg, "densityDpi"), Value::Int(240));
    assert_eq!(e.vm.get_field(cfg, "fontScale"), Value::Float(1.0));
    assert_eq!(
        int(e.call(
            res,
            "getIdentifier",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
            &[e.string("a"), e.string("b"), e.string("c")]
        )),
        0
    );
    let portrait = Env::with(|c| c.platform = Platform { screen_width: 480, screen_height: 800, ..test_platform() });
    let mut portrait = portrait;
    let pa = portrait.activity();
    let pres = obj(portrait.call(pa, "getResources", "()Landroid/content/res/Resources;", &[]));
    let cfg = obj(portrait.call(pres, "getConfiguration", "()Landroid/content/res/Configuration;", &[]));
    assert_eq!(portrait.vm.get_field(cfg, "orientation"), Value::Int(1));
    // Assets.
    let am = obj(e.call(res, "getAssets", "()Landroid/content/res/AssetManager;", &[]));
    let am2 = obj(e.call(app, "getAssets", "()Landroid/content/res/AssetManager;", &[]));
    assert_eq!(am, am2);
    let st = obj(e.call(am, "open", "(Ljava/lang/String;)Ljava/io/InputStream;", &[e.string("hello.txt")]));
    let buf = e.vm.new_prim_array(Ty::Byte, 64);
    let n = int(e.call(st, "read", "([B)I", &[Value::Ref(Some(buf))]));
    assert_eq!(n, 12);
    let st2 =
        obj(e.call(am, "open", "(Ljava/lang/String;I)Ljava/io/InputStream;", &[e.string("/dir/a.bin"), Value::Int(2)]));
    assert_eq!(int(e.call(st2, "available", "()I", &[])), 5);
    let r = e.call(am, "open", "(Ljava/lang/String;)Ljava/io/InputStream;", &[e.string("missing")]);
    assert_eq!(exc_class(&e, r), "java/io/FileNotFoundException");
    let l = obj(e.call(am, "list", "(Ljava/lang/String;)[Ljava/lang/String;", &[e.string("dir/")]));
    let Some(Data::Objs(items)) = e.vm.data(l) else { panic!() };
    assert_eq!(items.len(), 2);
    let l = obj(e.call(am, "list", "(Ljava/lang/String;)[Ljava/lang/String;", &[e.string("")]));
    assert_eq!(e.vm.array_len(l), Some(2));
    assert_eq!(
        e.call(am, "list", "(Ljava/lang/String;)[Ljava/lang/String;", &[e.string("nope")]),
        Ok(Value::Ref(None))
    );
    e.call(am, "close", "()V", &[]).unwrap();
}

#[test]
fn build_properties_and_misc_android() {
    let mut e = Env::new();
    let build = e.vm.class_named("android/os/Build").unwrap();
    let ver = e.vm.class_named("android/os/Build$VERSION").unwrap();
    let get = |e: &Env, k: ClassId, n: &str| e.vm.string(e.vm.get_static(k, n).as_ref().unwrap()).unwrap();
    assert_eq!(get(&e, build, "MODEL"), "bhpc");
    assert_eq!(get(&e, build, "CPU_ABI"), "armeabi-v7a");
    assert_eq!(get(&e, build, "MANUFACTURER"), "bhpc");
    assert_eq!(get(&e, ver, "RELEASE"), "4.4.4");
    assert_eq!(get(&e, ver, "SDK"), "19");
    assert_eq!(e.vm.get_static(ver, "SDK_INT"), Value::Int(19));
    // Process / SystemClock / Environment.
    assert_eq!(int(e.scall("android/os/Process", "myPid", "()I", &[])), std::process::id() as i32);
    assert_eq!(int(e.scall("android/os/Process", "myTid", "()I", &[])), 1);
    assert_eq!(int(e.scall("android/os/Process", "myUid", "()I", &[])), 10000);
    e.scall("android/os/Process", "setThreadPriority", "(I)V", &[Value::Int(-4)]).unwrap();
    for m in ["uptimeMillis", "elapsedRealtime", "currentThreadTimeMillis"] {
        assert_eq!(e.scall("android/os/SystemClock", m, "()J", &[]), Ok(Value::Long(1000)));
    }
    assert_eq!(e.scall("android/os/SystemClock", "elapsedRealtimeNanos", "()J", &[]), Ok(Value::Long(1_000_000_000)));
    e.scall("android/os/SystemClock", "sleep", "(J)V", &[Value::Long(1)]).unwrap();
    e.scall("android/os/SystemClock", "sleep", "(J)V", &[Value::Long(-1)]).unwrap();
    let ext = obj(e.scall("android/os/Environment", "getExternalStorageDirectory", "()Ljava/io/File;", &[]));
    assert_eq!(text!(e, e.call(ext, "getPath", "()Ljava/lang/String;", &[])), Platform::default().external_dir);
    let pubd = obj(e.scall(
        "android/os/Environment",
        "getExternalStoragePublicDirectory",
        "(Ljava/lang/String;)Ljava/io/File;",
        &[e.string("Download")],
    ));
    assert!(text!(e, e.call(pubd, "getPath", "()Ljava/lang/String;", &[])).ends_with("/sdcard/Download"));
    assert_eq!(
        text!(e, e.scall("android/os/Environment", "getExternalStorageState", "()Ljava/lang/String;", &[])),
        "mounted"
    );
    let dd = obj(e.scall("android/os/Environment", "getDataDirectory", "()Ljava/io/File;", &[]));
    assert_eq!(text!(e, e.call(dd, "getPath", "()Ljava/lang/String;", &[])), "/data");
    let rd = obj(e.scall("android/os/Environment", "getRootDirectory", "()Ljava/io/File;", &[]));
    assert_eq!(text!(e, e.call(rd, "getPath", "()Ljava/lang/String;", &[])), "/system");
    let envc = e.vm.class_named("android/os/Environment").unwrap();
    assert_eq!(e.vm.string(e.vm.get_static(envc, "DIRECTORY_MUSIC").as_ref().unwrap()).as_deref(), Some("Music"));
    // Log.
    for (m, lvl) in [("v", 'V'), ("d", 'D'), ("i", 'I'), ("w", 'W'), ("e", 'E')] {
        assert_eq!(
            int(e.scall(
                "android/util/Log",
                m,
                "(Ljava/lang/String;Ljava/lang/String;)I",
                &[e.string("tag"), e.string("msg")]
            )),
            0
        );
        assert!(e.log_text().contains(&format!("{lvl}/tag: msg")));
    }
    // Settings.Secure
    let act = e.activity();
    let cr = obj(e.call(act, "getContentResolver", "()Landroid/content/ContentResolver;", &[]));
    let sig = "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;";
    assert_eq!(
        text!(
            e,
            e.scall(
                "android/provider/Settings$Secure",
                "getString",
                sig,
                &[Value::Ref(Some(cr)), e.string("android_id")]
            )
        ),
        "0123456789abcdef"
    );
    assert_eq!(
        e.scall("android/provider/Settings$Secure", "getString", sig, &[Value::Ref(Some(cr)), e.string("other")]),
        Ok(Value::Ref(None))
    );
    assert_eq!(
        e.scall("android/provider/Settings$Secure", "getString", sig, &[Value::Ref(Some(cr)), Value::Ref(None)]),
        Ok(Value::Ref(None))
    );
    // PackageManager.
    let act = e.activity();
    let pm = obj(e.call(act, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[]));
    for (f, want) in [
        ("android.hardware.touchscreen", 1),
        ("android.hardware.touchscreen.multitouch", 1),
        ("android.hardware.screen.portrait", 1),
        ("android.hardware.camera", 0),
        ("android.hardware.sensor.gyroscope", 0),
    ] {
        assert_eq!(int(e.call(pm, "hasSystemFeature", "(Ljava/lang/String;)Z", &[e.string(f)])), want, "{f}");
    }
    assert_eq!(int(e.call(pm, "hasSystemFeature", "(Ljava/lang/String;)Z", &[Value::Ref(None)])), 0);
    let pi = obj(e.call(
        pm,
        "getPackageInfo",
        "(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;",
        &[e.string("x"), Value::Int(0)],
    ));
    assert_eq!(e.vm.string(e.vm.get_field(pi, "versionName").as_ref().unwrap()).as_deref(), Some("1.7.4"));
    assert_eq!(e.vm.get_field(pi, "versionCode"), Value::Int(0));
}

#[test]
fn bundle_and_preferences() {
    let mut e = Env::new();
    let b = e.new_obj("android/os/Bundle", "()V", &[]);
    assert_eq!(e.call(b, "containsKey", "(Ljava/lang/String;)Z", &[e.string("k")]), Ok(Value::Int(0)));
    e.call(b, "putString", "(Ljava/lang/String;Ljava/lang/String;)V", &[e.string("k"), e.string("v")]).unwrap();
    e.call(b, "putString", "(Ljava/lang/String;Ljava/lang/String;)V", &[e.string("k"), e.string("v2")]).unwrap();
    e.call(b, "putInt", "(Ljava/lang/String;I)V", &[e.string("i"), Value::Int(7)]).unwrap();
    e.call(b, "putBoolean", "(Ljava/lang/String;Z)V", &[e.string("z"), Value::Int(1)]).unwrap();
    assert_eq!(int(e.call(b, "size", "()I", &[])), 3);
    assert_eq!(text!(e, e.call(b, "getString", "(Ljava/lang/String;)Ljava/lang/String;", &[e.string("k")])), "v2");
    assert_eq!(
        e.call(b, "getString", "(Ljava/lang/String;)Ljava/lang/String;", &[e.string("none")]),
        Ok(Value::Ref(None))
    );
    assert_eq!(int(e.call(b, "getInt", "(Ljava/lang/String;)I", &[e.string("i")])), 7);
    assert_eq!(int(e.call(b, "getInt", "(Ljava/lang/String;)I", &[e.string("none")])), 0);
    assert_eq!(int(e.call(b, "getInt", "(Ljava/lang/String;I)I", &[e.string("none"), Value::Int(9)])), 9);
    assert_eq!(int(e.call(b, "getBoolean", "(Ljava/lang/String;)Z", &[e.string("z")])), 1);
    assert_eq!(int(e.call(b, "getBoolean", "(Ljava/lang/String;Z)Z", &[e.string("none"), Value::Int(1)])), 1);
    assert_eq!(e.call(b, "containsKey", "(Ljava/lang/String;)Z", &[Value::Ref(None)]), Ok(Value::Int(0)));

    let app = e.activity();
    let prefs = obj(e.call(
        app,
        "getSharedPreferences",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        &[e.string("p"), Value::Int(0)],
    ));
    let again = obj(e.call(
        app,
        "getSharedPreferences",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        &[e.string("p"), Value::Int(0)],
    ));
    assert_eq!(prefs, again);
    let other = obj(e.call(
        app,
        "getSharedPreferences",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        &[e.string("q"), Value::Int(0)],
    ));
    assert_ne!(prefs, other);
    let ed = obj(e.call(prefs, "edit", "()Landroid/content/SharedPreferences$Editor;", &[]));
    let ret = "Landroid/content/SharedPreferences$Editor;";
    e.call(ed, "putString", &format!("(Ljava/lang/String;Ljava/lang/String;){ret}"), &[e.string("s"), e.string("str")])
        .unwrap();
    e.call(ed, "putInt", &format!("(Ljava/lang/String;I){ret}"), &[e.string("i"), Value::Int(3)]).unwrap();
    e.call(ed, "putLong", &format!("(Ljava/lang/String;J){ret}"), &[e.string("l"), Value::Long(1 << 40)]).unwrap();
    e.call(ed, "putFloat", &format!("(Ljava/lang/String;F){ret}"), &[e.string("f"), Value::Float(0.5)]).unwrap();
    e.call(ed, "putBoolean", &format!("(Ljava/lang/String;Z){ret}"), &[e.string("b"), Value::Int(1)]).unwrap();
    // Nothing is visible before commit.
    assert_eq!(e.call(prefs, "contains", "(Ljava/lang/String;)Z", &[e.string("s")]), Ok(Value::Int(0)));
    assert_eq!(e.call(ed, "commit", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(
        text!(
            e,
            e.call(
                prefs,
                "getString",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[e.string("s"), e.string("d")]
            )
        ),
        "str"
    );
    assert_eq!(
        text!(
            e,
            e.call(
                prefs,
                "getString",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[e.string("zz"), e.string("d")]
            )
        ),
        "d"
    );
    assert_eq!(
        e.call(
            prefs,
            "getString",
            "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
            &[e.string("zz"), Value::Ref(None)]
        ),
        Ok(Value::Ref(None))
    );
    assert_eq!(int(e.call(prefs, "getInt", "(Ljava/lang/String;I)I", &[e.string("i"), Value::Int(0)])), 3);
    assert_eq!(int(e.call(prefs, "getInt", "(Ljava/lang/String;I)I", &[e.string("zz"), Value::Int(8)])), 8);
    assert_eq!(
        e.call(prefs, "getLong", "(Ljava/lang/String;J)J", &[e.string("l"), Value::Long(0)]),
        Ok(Value::Long(1 << 40))
    );
    assert_eq!(
        e.call(prefs, "getLong", "(Ljava/lang/String;J)J", &[e.string("zz"), Value::Long(5)]),
        Ok(Value::Long(5))
    );
    assert_eq!(
        e.call(prefs, "getFloat", "(Ljava/lang/String;F)F", &[e.string("f"), Value::Float(0.0)]),
        Ok(Value::Float(0.5))
    );
    assert_eq!(
        e.call(prefs, "getFloat", "(Ljava/lang/String;F)F", &[e.string("zz"), Value::Float(2.0)]),
        Ok(Value::Float(2.0))
    );
    assert_eq!(int(e.call(prefs, "getBoolean", "(Ljava/lang/String;Z)Z", &[e.string("b"), Value::Int(0)])), 1);
    let all = obj(e.call(prefs, "getAll", "()Ljava/util/Map;", &[]));
    assert_eq!(int(e.call(all, "size", "()I", &[])), 5);
    // The editor is reusable: remove and clear.
    e.call(ed, "remove", &format!("(Ljava/lang/String;){ret}"), &[e.string("s")]).unwrap();
    e.call(ed, "apply", "()V", &[]).unwrap();
    assert_eq!(e.call(prefs, "contains", "(Ljava/lang/String;)Z", &[e.string("s")]), Ok(Value::Int(0)));
    assert_eq!(int(e.call(all, "size", "()I", &[])), 5, "getAll returns a snapshot");
    e.call(ed, "clear", &format!("(){ret}"), &[]).unwrap();
    e.call(ed, "putInt", &format!("(Ljava/lang/String;I){ret}"), &[e.string("after"), Value::Int(1)]).unwrap();
    assert_eq!(e.call(ed, "commit", "()Z", &[]), Ok(Value::Int(1)));
    let all = obj(e.call(prefs, "getAll", "()Ljava/util/Map;", &[]));
    assert_eq!(int(e.call(all, "size", "()I", &[])), 1);
    // An editor with no preferences object cannot commit.
    let orphan = e.new_obj("android/app/SharedPreferencesImpl$EditorImpl", "()V", &[]);
    assert_eq!(e.call(orphan, "commit", "()Z", &[]), Ok(Value::Int(0)));
    // A preferences object that is not backed by a map ignores edits.
    let weird = e.vm.alloc_object(e.vm.class_named("android/app/SharedPreferencesImpl").unwrap());
    e.vm.with_data(weird, |d| *d = Data::None);
    assert_eq!(int(e.call(weird, "contains", "(Ljava/lang/String;)Z", &[e.string("x")])), 0);
    let all = obj(e.call(weird, "getAll", "()Ljava/util/Map;", &[]));
    assert_eq!(int(e.call(all, "size", "()I", &[])), 0);
    // Editors that lost their map or whose preferences did commit nothing.
    let ed2 = obj(e.call(weird, "edit", "()Landroid/content/SharedPreferences$Editor;", &[]));
    e.vm.with_data(ed2, |d| *d = Data::None);
    assert_eq!(e.call(ed2, "commit", "()Z", &[]), Ok(Value::Int(1)));
    let ed3 = obj(e.call(weird, "edit", "()Landroid/content/SharedPreferences$Editor;", &[]));
    e.call(
        ed3,
        "putInt",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences$Editor;",
        &[e.string("k"), Value::Int(1)],
    )
    .unwrap();
    assert_eq!(e.call(ed3, "commit", "()Z", &[]), Ok(Value::Int(1)));
    // A bundle that lost its map.
    let hollow = e.vm.alloc_object(e.vm.class_named("android/os/Bundle").unwrap());
    e.vm.with_data(hollow, |d| *d = Data::None);
    e.call(hollow, "putString", "(Ljava/lang/String;Ljava/lang/String;)V", &[e.string("k"), e.string("v")]).unwrap();
    assert_eq!(int(e.call(hollow, "size", "()I", &[])), 0);
    assert_eq!(int(e.call(hollow, "containsKey", "(Ljava/lang/String;)Z", &[e.string("x")])), 0);
}

#[test]
fn verde_activity() {
    let mut e = Env::new();
    let a = e.activity();
    assert_eq!(e.activity(), a);
    assert!(e.vm.instance_of(a, "android/app/Activity"));
    assert_eq!(
        text!(e, e.scall("com/apportable/activity/VerdeActivity", "androidId", "()Ljava/lang/String;", &[])),
        "0123456789abcdef"
    );
    for (m, want) in [
        ("getScreenWidth", 800),
        ("getScreenHeight", 480),
        ("getContentWidth", 800),
        ("getContentHeight", 480),
        ("getSplashScreenWidth", 800),
        ("getSplashScreenHeight", 480),
        ("_getRequestedOrientation", 0),
    ] {
        assert_eq!(int(e.call(a, m, "()I", &[])), want, "{m}");
    }
    for (m, want) in [("canSendEmail", 1), ("hasWindowFocus", 1), ("isFinishing", 0)] {
        assert_eq!(int(e.call(a, m, "()Z", &[])).min(1), want, "{m}");
    }
    assert_eq!(
        e.call(a, "hasActivityForAction", "(Ljava/lang/String;)Z", &[e.string("android.intent.action.VIEW")]),
        Ok(Value::Int(1))
    );
    assert_eq!(
        int(e.call(a, "hasSystemFeature", "(Ljava/lang/String;)I", &[e.string("android.hardware.touchscreen")])),
        1
    );
    assert_eq!(int(e.call(a, "hasSystemFeature", "(Ljava/lang/String;)I", &[e.string("android.hardware.camera")])), 0);
    assert_eq!(int(e.call(a, "hasSystemFeature", "(Ljava/lang/String;)I", &[Value::Ref(None)])), 0);
    assert_eq!(e.call(a, "getAttributionId", "()Ljava/lang/String;", &[]), Ok(Value::Ref(None)));
    assert_eq!(text!(e, e.call(a, "uuid", "()Ljava/lang/String;", &[])), "00000000-0000-4000-8000-0123456789ab");
    for m in ["gc", "homeScreen", "finish"] {
        e.call(a, m, "()V", &[]).unwrap();
    }
    e.call(a, "_setRequestedOrientation", "(I)V", &[Value::Int(0)]).unwrap();
    e.call(a, "sendEmail", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V", &[Value::Ref(None); 8]).unwrap();
    let c1 = obj(e.call(a, "primaryContainer", "()Landroid/widget/FrameLayout;", &[]));
    assert_eq!(obj(e.call(a, "primaryContainer", "()Landroid/widget/FrameLayout;", &[])), c1);
    // terminateProcess stops the CPU via the JNI path.
    let ch = e.class_h("com/apportable/activity/VerdeActivity");
    let n = e.cstr("terminateProcess");
    let s = e.cstr("()V");
    let mid = e.jni("GetStaticMethodID", &[ch, n, s]).r0;
    assert_eq!(
        e.try_jni("CallStaticVoidMethodA", &[ch, mid, 0]),
        Err(Stop::Host("VerdeActivity.terminateProcess".into()))
    );
    let n = e.cstr("killProcess");
    let s = e.cstr("(I)V");
    let pc = e.class_h("android/os/Process");
    let mid = e.jni("GetStaticMethodID", &[pc, n, s]).r0;
    let arg = e.buf(8);
    assert_eq!(e.try_jni("CallStaticVoidMethodA", &[pc, mid, arg]), Err(Stop::Host("Process.killProcess".into())));
}

#[derive(Default)]
struct Gfx {
    log: Mutex<Vec<String>>,
}

impl GfxHooks for Gfx {
    fn swap_buffers(&self) -> bool {
        self.log.lock().unwrap().push("swap".into());
        false
    }
    fn make_current(&self, ctx: u32) -> bool {
        self.log.lock().unwrap().push(format!("current {ctx}"));
        true
    }
    fn create_shared_context(&self, version: i32) -> u32 {
        self.log.lock().unwrap().push(format!("shared {version}"));
        42
    }
    fn destroy_context(&self, ctx: u32) -> bool {
        self.log.lock().unwrap().push(format!("destroy {ctx}"));
        false
    }
    fn set_content_size(&self, w: f32, h: f32) {
        self.log.lock().unwrap().push(format!("size {w}x{h}"));
    }
    fn context_lost(&self) {
        self.log.lock().unwrap().push("lost".into());
    }
}

#[test]
fn egl_callbacks_reach_the_gfx_hooks() {
    let gfx = Arc::new(Gfx::default());
    let g2 = gfx.clone();
    let mut e = Env::with(|c| c.gfx = g2);
    let a = e.activity();
    let ctx = "Ljavax/microedition/khronos/egl/EGLContext;";
    let primary = obj(e.call(a, "getPrimaryContext", &format!("(){ctx}"), &[]));
    assert_eq!(e.vm.get_field(primary, "id"), Value::Int(1));
    assert_eq!(obj(e.call(a, "getPrimaryContext", &format!("(){ctx}"), &[])), primary);
    assert_eq!(e.call(a, "makeCurrentContext", &format!("({ctx})Z"), &[Value::Ref(Some(primary))]), Ok(Value::Int(1)));
    assert_eq!(e.call(a, "makeCurrentContext", &format!("({ctx})Z"), &[Value::Ref(None)]), Ok(Value::Int(1)));
    let shared =
        obj(e.call(a, "createSharedContext", &format!("(I{ctx}){ctx}"), &[Value::Int(2), Value::Ref(Some(primary))]));
    assert_eq!(e.vm.get_field(shared, "id"), Value::Int(42));
    assert_eq!(e.call(a, "destroyContext", &format!("({ctx})Z"), &[Value::Ref(Some(shared))]), Ok(Value::Int(0)));
    assert_eq!(e.call(a, "swapBuffers", "()Z", &[]), Ok(Value::Int(0)));
    e.call(a, "setContentSize", "(FF)V", &[Value::Float(320.0), Value::Float(480.0)]).unwrap();
    e.call(a, "rektEGLContext", "()V", &[]).unwrap();
    assert_eq!(
        gfx.log.lock().unwrap().as_slice(),
        ["current 1", "current 0", "shared 2", "destroy 42", "swap", "size 320x480", "lost"]
    );
    // The defaults succeed silently.
    let mut d = Env::new();
    let a = d.activity();
    assert_eq!(d.call(a, "swapBuffers", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(d.call(a, "makeCurrentContext", &format!("({ctx})Z"), &[Value::Ref(None)]), Ok(Value::Int(1)));
    assert_eq!(d.call(a, "destroyContext", &format!("({ctx})Z"), &[Value::Ref(None)]), Ok(Value::Int(1)));
    let s = obj(d.call(a, "createSharedContext", &format!("(I{ctx}){ctx}"), &[Value::Int(2), Value::Ref(None)]));
    assert_eq!(d.vm.get_field(s, "id"), Value::Int(2));
    d.call(a, "setContentSize", "(FF)V", &[Value::Float(1.0), Value::Float(1.0)]).unwrap();
    d.call(a, "rektEGLContext", "()V", &[]).unwrap();
    let _ = NoGfx;
}

#[test]
fn main_thread_and_run_loop() {
    let mut e = Env::new();
    let mt = "com/apportable/MainThread";
    assert_eq!(e.scall(mt, "isStarted", "()Z", &[]), Ok(Value::Int(1)));
    e.scall(mt, "start", "()V", &[]).unwrap();
    e.scall(mt, "goNative", "()V", &[]).unwrap();
    let h = obj(e.scall(mt, "getHandler", "()Landroid/os/Handler;", &[]));
    assert_eq!(obj(e.scall(mt, "getHandler", "()Landroid/os/Handler;", &[])), h);
    assert_eq!(e.vm.get_field(h, "looper"), Value::Ref(Some(e.vm.main_looper())));
    // Before the main looper is attached to a thread, its thread is thread 0.
    let t = obj(e.scall(mt, "getThread", "()Ljava/lang/Thread;", &[]));
    assert_eq!(text!(e, e.call(t, "getName", "()Ljava/lang/String;", &[])), "main");
    // Tick actions.
    let counter = Arc::new(Mutex::new(0));
    let c2 = counter.clone();
    e.vm.define("t/Tick").implements(&["java/lang/Runnable"]).method("run", "()V", move |_| {
        *c2.lock().unwrap() += 1;
        Ok(Value::Void)
    });
    let tick = e.new_obj("t/Tick", "()V", &[]);
    e.scall(mt, "addTickAction", "(Ljava/lang/Runnable;)V", &[Value::Ref(Some(tick))]).unwrap();
    e.scall(mt, "addTickAction", "(Ljava/lang/Runnable;)V", &[Value::Ref(None)]).unwrap();
    e.scall(mt, "tick", "()V", &[]).unwrap();
    e.scall(mt, "tick", "()V", &[]).unwrap();
    assert_eq!(*counter.lock().unwrap(), 2);
    e.scall(mt, "removeTickAction", "(Ljava/lang/Runnable;)V", &[Value::Ref(Some(tick))]).unwrap();
    e.scall(mt, "tick", "()V", &[]).unwrap();
    assert_eq!(*counter.lock().unwrap(), 2);
    // RunLoop.
    let rl = "com/apportable/utils/RunLoop";
    assert_eq!(e.scall(rl, "isUnifiedThreadModel", "()Z", &[]), Ok(Value::Int(1)));
    e.scall(rl, "prepare", "()V", &[]).unwrap();
    let ml = e.vm.main_looper();
    let t0 = obj(e.scall(mt, "getThread", "()Ljava/lang/Thread;", &[]));
    assert_eq!(t0, crate::jlib::thread_object(&e.vm, e.cpu.processor_id()));
    let tick2 = e.new_obj("t/Tick", "()V", &[]);
    assert!(e.vm.post_main(tick2, 0));
    assert_eq!(e.vm.pending_messages(ml), 1);
    e.scall(rl, "processMessages", "()V", &[]).unwrap();
    assert_eq!(*counter.lock().unwrap(), 3);
    assert_eq!(e.vm.pending_messages(ml), 0);
    // A failing tick action propagates.
    e.vm.define("t/BadTick")
        .implements(&["java/lang/Runnable"])
        .method("run", "()V", |c| Err(c.vm.throw_new("java/lang/RuntimeException", "tick")));
    let bad = e.new_obj("t/BadTick", "()V", &[]);
    e.scall(mt, "addTickAction", "(Ljava/lang/Runnable;)V", &[Value::Ref(Some(bad))]).unwrap();
    let r = e.scall(mt, "tick", "()V", &[]);
    assert_eq!(exc_class(&e, r), "java/lang/RuntimeException");
}

#[test]
fn lifecycle_device_and_declared_natives() {
    let mut e = Env::new();
    let lc = "com/apportable/Lifecycle";
    assert_eq!(e.scall(lc, "isCreated", "()Z", &[]), Ok(Value::Int(1)));
    assert_eq!(e.scall(lc, "isInBackground", "()Z", &[]), Ok(Value::Int(0)));
    assert_eq!(e.scall(lc, "isNativeApplicationStarted", "()Z", &[]), Ok(Value::Int(1)));
    // Declared natives are reported stubs until bound.
    e.scall(lc, "contextDidBecomeValid", "()V", &[]).unwrap();
    e.scall("com/apportable/activity/VerdeActivity", "nativeOnCreate", "()V", &[]).unwrap();
    e.scall("com/apportable/MainThread", "nativeRun", "()V", &[]).unwrap();
    let rep = e.vm.report();
    assert!(rep.stubs().any(|m| m.class == lc && m.name == "contextDidBecomeValid"));
    assert_eq!(
        text!(e, e.scall("com/apportable/ui/Device", "macAddress", "()Ljava/lang/String;", &[])),
        "02:00:00:00:00:00"
    );
    assert_eq!(e.scall("com/apportable/ui/Device", "isAutorotationOn", "()Z", &[]), Ok(Value::Int(0)));
    let d = obj(e.scall(
        "com/apportable/ui/Device",
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/Device;",
        &[Value::Ref(None), Value::Int(0)],
    ));
    e.call(d, "enable", "()V", &[]).unwrap();
    e.call(d, "disable", "()V", &[]).unwrap();
}

#[test]
fn queue_event_and_save_preferences() {
    let mut e = Env::new();
    let a = e.activity();
    let ran = Arc::new(Mutex::new(0));
    let r2 = ran.clone();
    e.vm.define("t/Job").implements(&["java/lang/Runnable"]).method("run", "()V", move |_| {
        *r2.lock().unwrap() += 1;
        Ok(Value::Void)
    });
    let job = e.new_obj("t/Job", "()V", &[]);
    e.call(a, "queueEvent", "(Ljava/lang/Runnable;)V", &[Value::Ref(Some(job))]).unwrap();
    e.call(a, "runOnThread", "(Ljava/lang/Runnable;I)V", &[Value::Ref(Some(job)), Value::Int(0)]).unwrap();
    e.call(a, "runOnUiThread", "(Ljava/lang/Runnable;)V", &[Value::Ref(Some(job))]).unwrap();
    e.call(a, "queueEvent", "(Ljava/lang/Runnable;)V", &[Value::Ref(None)]).unwrap();
    e.call(a, "runOnUiThread", "(Ljava/lang/Runnable;)V", &[Value::Ref(None)]).unwrap();
    let ml = e.vm.main_looper();
    assert_eq!(e.vm.pending_messages(ml), 3);
    let now = e.vm.now_ms();
    let vm = e.vm.clone();
    assert_eq!(vm.pump(&mut e.cpu, ml, now), Ok(3));
    assert_eq!(*ran.lock().unwrap(), 3);
    // savePreferenceEditorHack commits the editor.
    let prefs = obj(e.call(
        a,
        "getSharedPreferences",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        &[e.string("p"), Value::Int(0)],
    ));
    let ed = obj(e.call(prefs, "edit", "()Landroid/content/SharedPreferences$Editor;", &[]));
    e.call(
        ed,
        "putInt",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences$Editor;",
        &[e.string("k"), Value::Int(1)],
    )
    .unwrap();
    e.call(a, "savePreferenceEditorHack", "(Landroid/content/SharedPreferences$Editor;)V", &[Value::Ref(Some(ed))])
        .unwrap();
    assert_eq!(int(e.call(prefs, "getInt", "(Ljava/lang/String;I)I", &[e.string("k"), Value::Int(0)])), 1);
    e.call(a, "savePreferenceEditorHack", "(Landroid/content/SharedPreferences$Editor;)V", &[Value::Ref(None)])
        .unwrap();
}

#[test]
fn collections_without_payload_degrade_gracefully() {
    let mut e = Env::new();
    let l = e.vm.alloc_object(e.vm.class_named("java/util/ArrayList").unwrap());
    e.vm.with_data(l, |d| *d = Data::None);
    assert_eq!(int(e.call(l, "size", "()I", &[])), 0);
    assert_eq!(e.call(l, "isEmpty", "()Z", &[]), Ok(Value::Int(1)));
    let r = e.call(l, "get", "(I)Ljava/lang/Object;", &[Value::Int(0)]);
    assert_eq!(exc_class(&e, r), "java/lang/IndexOutOfBoundsException");
    let m = e.vm.alloc_object(e.vm.class_named("java/util/HashMap").unwrap());
    e.vm.with_data(m, |d| *d = Data::None);
    assert_eq!(int(e.call(m, "size", "()I", &[])), 0);
    assert_eq!(e.call(m, "containsKey", "(Ljava/lang/Object;)Z", &[Value::Ref(None)]), Ok(Value::Int(0)));
    let msg = e.new_obj("android/os/Message", "()V", &[]);
    assert_eq!(e.vm.get_field(msg, "what"), Value::Int(0));
}

#[test]
fn ui_widgets_are_created_and_alerts_are_logged() {
    let mut e = Env::new();
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let l = log.clone();
    e.vm.set_log(move |m| l.lock().unwrap().push(m.to_string()));
    let ctx = Value::Ref(Some(e.vm.activity()));
    for w in crate::jlib::WIDGETS {
        let class = format!("com/apportable/ui/{w}");
        let owner = if w == "MediaPlayer" { "Landroid/app/Activity;" } else { "Landroid/content/Context;" };
        let a = obj(e.scall(&class, "create", &format!("({owner}I)L{class};"), &[ctx, Value::Int(7)]));
        assert_eq!(e.vm.get_field(a, "mObject").as_int(), 7, "{w}");
        let rect = Value::Ref(None);
        let b = obj(e.scall(
            &class,
            "create",
            &format!("({owner}ILandroid/graphics/RectF;)L{class};"),
            &[ctx, Value::Int(8), rect],
        ));
        assert_ne!(a, b);
    }
    let alert = obj(e.scall(
        "com/apportable/ui/AlertView",
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/AlertView;",
        &[ctx, Value::Int(1)],
    ));
    let (t, m) = (e.string("Title"), e.string("Body"));
    e.call(alert, "setTitle", "(Ljava/lang/String;)V", &[t]).unwrap();
    e.call(alert, "setMessage", "(Ljava/lang/String;)V", &[m]).unwrap();
    let log = log.lock().unwrap();
    assert!(log.iter().any(|l| l.contains("alert title") && l.contains("Title")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("alert message") && l.contains("Body")), "{log:?}");
}

#[test]
fn layout_pass_calls_native_layout_subviews_of_marked_views() {
    let mut e = Env::new();
    let ctx = Value::Ref(Some(e.vm.activity()));
    let w = obj(e.scall(
        "com/apportable/ui/Window",
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/Window;",
        &[ctx, Value::Int(41)],
    ));
    let v = obj(e.scall(
        "com/apportable/ui/View",
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/View;",
        &[ctx, Value::Int(42)],
    ));
    // Creation marks both; the unbound native is a reported stub, so the pass only counts them.
    assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(2));
    assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(0), "clean views are skipped");
    let rect = Value::Ref(None);
    for (name, sig, args) in [
        ("setFrame", "(Landroid/graphics/RectF;)V", vec![rect]),
        ("setBounds", "(Landroid/graphics/RectF;)V", vec![rect]),
        ("setNeedsLayout", "()V", vec![]),
        ("layoutIfNeeded", "()V", vec![]),
    ] {
        e.call(v, name, sig, &args).unwrap();
        assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(1), "{name}");
    }
    e.call(w, "addSubview", "(Lcom/apportable/ui/View;)V", &[Value::Ref(Some(v))]).unwrap();
    assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(2));
    e.call(w, "makeKeyAndOrderFront", "()V", &[]).unwrap();
    assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(1));
    assert!(e.vm.report().members.iter().any(|m| m.name == "nativeOnWindowFocusChanged" && m.calls == 1));
    assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(0));
    assert!(e.vm.report().members.iter().any(|m| m.name == "nativeOnWindowFocusChanged" && m.calls == 1), "once");
    // A bound native receives the view's native object.
    let view_class = e.vm.class_named("com/apportable/ui/View").unwrap();
    let ret = e.code(&[0xE12F_FF1E]);
    e.vm.register_native(view_class, "callLayoutSubviews", "(I)V", ret);
    e.call(v, "setNeedsLayout", "()V", &[]).unwrap();
    assert_eq!(e.vm.run_layout(&mut e.cpu), Ok(1));
}

struct RecordingAudio(Mutex<Vec<String>>);

impl crate::AudioHooks for RecordingAudio {
    fn min_buffer_size(&self, rate: i32, ch: i32, enc: i32) -> i32 {
        self.0.lock().unwrap().push(format!("min {rate} {ch} {enc}"));
        4096
    }
    fn create(&self, rate: i32, ch: i32, enc: i32, buf: i32) -> Option<u32> {
        self.0.lock().unwrap().push(format!("create {rate} {ch} {enc} {buf}"));
        (rate > 0).then_some(5)
    }
    fn control(&self, id: u32, op: crate::TrackOp) -> i32 {
        self.0.lock().unwrap().push(format!("{id} {op:?}"));
        0
    }
    fn write(&self, _cpu: &mut bhpc_cpu::Cpu, id: u32, data: &[u8]) -> i32 {
        self.0.lock().unwrap().push(format!("write {id} {data:?}"));
        data.len() as i32
    }
    fn head_position(&self, id: u32) -> i32 {
        id as i32 * 100
    }
}

#[test]
fn audio_track_delegates_to_the_audio_hooks() {
    let rec = Arc::new(RecordingAudio(Mutex::new(Vec::new())));
    let r2 = rec.clone();
    let mut e = Env::with(move |c| c.audio = r2);
    let min = e.scall(
        "android/media/AudioTrack",
        "getMinBufferSize",
        "(III)I",
        &[Value::Int(44100), Value::Int(3), Value::Int(2)],
    );
    assert_eq!(int(min), 4096);
    assert_eq!(int(e.scall("android/media/AudioTrack", "getNativeOutputSampleRate", "(I)I", &[Value::Int(3)])), 44100);
    let args = [Value::Int(3), Value::Int(44100), Value::Int(3), Value::Int(2), Value::Int(8192), Value::Int(1)];
    let t = e.new_obj("android/media/AudioTrack", "(IIIIII)V", &args);
    assert_eq!(int(e.call(t, "getState", "()I", &[])), 1);
    for m in ["play", "pause", "stop", "flush"] {
        e.call(t, m, "()V", &[]).unwrap();
    }
    assert_eq!(int(e.call(t, "setStereoVolume", "(FF)I", &[Value::Float(0.25), Value::Float(0.5)])), 0);
    assert_eq!(int(e.call(t, "setVolume", "(F)I", &[Value::Float(0.75)])), 0);
    assert_eq!(int(e.call(t, "getPlaybackHeadPosition", "()I", &[])), 500);
    let arr = e.vm.new_byte_array(&[1, 2, 3, 4, 5, 6]);
    let w = e.call(t, "write", "([BII)I", &[Value::Ref(Some(arr)), Value::Int(2), Value::Int(3)]);
    assert_eq!(int(w), 3);
    let w = e.call(t, "write", "([BII)I", &[Value::Ref(Some(arr)), Value::Int(4), Value::Int(100)]);
    assert_eq!(int(w), 2, "clamped to the array");
    assert_eq!(int(e.call(t, "write", "([BII)I", &[Value::Ref(None), Value::Int(0), Value::Int(1)])), -2);
    e.call(t, "release", "()V", &[]).unwrap();
    assert_eq!(int(e.call(t, "getState", "()I", &[])), 0, "released tracks are uninitialized");
    let bad = e.new_obj(
        "android/media/AudioTrack",
        "(IIIIII)V",
        &[Value::Int(3), Value::Int(0), Value::Int(3), Value::Int(2), Value::Int(1), Value::Int(1)],
    );
    assert_eq!(int(e.call(bad, "getState", "()I", &[])), 0);
    let log = rec.0.lock().unwrap().join("|");
    for want in [
        "min 44100 3 2",
        "create 44100 3 2 8192",
        "5 Play",
        "5 Pause",
        "5 Stop",
        "5 Flush",
        "5 Volume(0.5)",
        "5 Volume(0.75)",
        "write 5 [3, 4, 5]",
        "write 5 [5, 6]",
        "5 Release",
    ] {
        assert!(log.contains(want), "{want} in {log}");
    }
}

#[test]
fn the_default_audio_back_end_paces_writes() {
    let mut e = Env::new();
    let t = e.new_obj(
        "android/media/AudioTrack",
        "(IIIIII)V",
        &[Value::Int(3), Value::Int(44100), Value::Int(3), Value::Int(2), Value::Int(8192), Value::Int(1)],
    );
    assert_eq!(int(e.call(t, "getState", "()I", &[])), 1);
    assert_eq!(
        int(e.scall(
            "android/media/AudioTrack",
            "getMinBufferSize",
            "(III)I",
            &[Value::Int(44100), Value::Int(3), Value::Int(2)]
        )),
        7056
    );
    assert_eq!(int(e.call(t, "play", "()V", &[]).map(|_| Value::Int(0))), 0);
    let arr = e.vm.new_byte_array(&vec![0u8; 1764]);
    let begun = std::time::Instant::now();
    assert_eq!(int(e.call(t, "write", "([BII)I", &[Value::Ref(Some(arr)), Value::Int(0), Value::Int(1764)])), 1764);
    assert!(begun.elapsed() >= std::time::Duration::from_millis(9), "10 ms of audio takes 10 ms to write");
    assert_eq!(int(e.call(t, "getPlaybackHeadPosition", "()I", &[])), 0);
}

#[test]
fn alerts_collect_buttons_and_click_through_the_natives() {
    let mut e = Env::new();
    let ctx = Value::Ref(Some(e.vm.activity()));
    let a = obj(e.scall(
        "com/apportable/ui/AlertView",
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/AlertView;",
        &[ctx, Value::Int(55)],
    ));
    let s = |e: &Env, t: &str| e.string(t);
    let (t, m, b0, b1, c) = (s(&e, "T"), s(&e, "M"), s(&e, "Yes"), s(&e, "No"), s(&e, "Cancel"));
    e.call(a, "setTitle", "(Ljava/lang/String;)V", &[t]).unwrap();
    e.call(a, "setMessage", "(Ljava/lang/String;)V", &[m]).unwrap();
    e.call(a, "addButtonWithTitle", "(Ljava/lang/String;I)V", &[b0, Value::Int(1)]).unwrap();
    e.call(a, "addButtonWithTitle", "(Ljava/lang/String;I)V", &[b1, Value::Int(2)]).unwrap();
    e.call(a, "setCancelButtonTitle", "(Ljava/lang/String;)V", &[c]).unwrap();
    e.call(a, "setCancelButtonIndex", "(I)V", &[Value::Int(0)]).unwrap();
    e.call(a, "setStyle", "(I)V", &[Value::Int(0)]).unwrap();
    assert!(e.vm.take_alerts().is_empty(), "not shown yet");
    e.call(a, "show", "()V", &[]).unwrap();
    let shown = e.vm.take_alerts();
    assert_eq!(shown.len(), 1);
    assert_eq!((shown[0].title.as_str(), shown[0].message.as_str()), ("T", "M"));
    assert_eq!(shown[0].buttons, [("Yes".to_string(), 1), ("No".to_string(), 2)]);
    assert_eq!(shown[0].cancel, Some(("Cancel".to_string(), 0)));
    assert!(e.vm.take_alerts().is_empty());
    // Setting the cancel index first, then the title keeps both.
    let b = obj(e.scall(
        "com/apportable/ui/AlertView",
        "create",
        "(Landroid/content/Context;I)Lcom/apportable/ui/AlertView;",
        &[ctx, Value::Int(56)],
    ));
    let c2 = s(&e, "Nope");
    e.call(b, "setCancelButtonIndex", "(I)V", &[Value::Int(3)]).unwrap();
    e.call(b, "setCancelButtonTitle", "(Ljava/lang/String;)V", &[c2]).unwrap();
    e.call(b, "show", "()V", &[]).unwrap();
    assert_eq!(e.vm.take_alerts()[0].cancel, Some(("Nope".to_string(), 3)));
    e.call(b, "dismiss", "()V", &[]).unwrap();
    // A click calls nativeOnClick(mObject, index) then nativeOnDismiss(mObject).
    e.vm.alert_click(&mut e.cpu, &shown[0], 1).unwrap();
    let report = e.vm.report();
    let calls = |n: &str| report.members.iter().find(|m| m.name == n).map(|m| m.calls);
    assert_eq!((calls("nativeOnClick"), calls("nativeOnDismiss")), (Some(1), Some(1)));
}

#[test]
fn screen_follows_the_host_and_content_size_is_remembered() {
    let mut e = Env::new();
    let a = e.activity();
    // Before the game announces a content size the content is the screen.
    assert_eq!(e.vm.screen_size(), (800, 480));
    assert_eq!(e.vm.content_size(), None);
    assert_eq!(e.vm.content_scale(), (1.0, 1.0));
    assert_eq!(e.call(a, "getContentWidth", "()I", &[]), Ok(Value::Int(800)));
    // The window grew: the game's next queries see it, the content size stays what the game chose.
    e.vm.set_screen_size(1280, 960);
    e.call(a, "setContentSize", "(FF)V", &[Value::Float(1024.0), Value::Float(768.0)]).unwrap();
    assert_eq!(e.call(a, "getScreenWidth", "()I", &[]), Ok(Value::Int(1280)));
    assert_eq!(e.call(a, "getScreenHeight", "()I", &[]), Ok(Value::Int(960)));
    assert_eq!(e.call(a, "getContentWidth", "()I", &[]), Ok(Value::Int(1024)));
    assert_eq!(e.call(a, "getContentHeight", "()I", &[]), Ok(Value::Int(768)));
    assert_eq!(e.vm.content_size(), Some((1024.0, 768.0)));
    assert_eq!(e.vm.content_scale(), (1.25, 1.25));
    // A degenerate announcement is ignored for scaling.
    e.call(a, "setContentSize", "(FF)V", &[Value::Float(0.0), Value::Float(0.0)]).unwrap();
    assert_eq!(e.vm.content_scale(), (1.0, 1.0));
}
