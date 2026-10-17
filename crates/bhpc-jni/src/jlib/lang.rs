//! `java.lang`.

use super::{boolean, int, long, object, string, void};
use crate::mutf8;
use crate::value::{Obj, Ty, Value};
use crate::vm::{Call, Data, JResult, Vm};
use bhpc_cpu::Stop;

fn str_of(c: &Call) -> Vec<u16> {
    c.vm.string_units(c.this).unwrap_or_default()
}

fn java_hash(u: &[u16]) -> i32 {
    u.iter().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(*c as i32))
}

fn append(c: &mut Call, text: &[u16]) -> JResult {
    c.vm.with_data(c.this, |d| {
        if let Data::Str(u) = d {
            u.extend_from_slice(text);
        }
    });
    object(Some(c.this))
}

/// `String.valueOf(Object)` of argument 0.
fn display(c: &Call, v: Value) -> String {
    match v.as_ref() {
        None => "null".to_string(),
        Some(o) => text_of(c.vm, o),
    }
}

/// `String.valueOf(Object)` for the objects the fake library knows.
pub(crate) fn text_of(vm: &Vm, o: Obj) -> String {
    if let Some(s) = vm.string(o) {
        return s;
    }
    if let Some(v) = vm.unbox(o) {
        let cls = vm.class_of(o).map(|c| vm.class_name(c)).unwrap_or_default();
        return match (cls.as_str(), v) {
            ("java/lang/Boolean", v) => (v.as_int() != 0).to_string(),
            ("java/lang/Character", v) => char::from_u32(v.as_int() as u32).unwrap_or('?').to_string(),
            (_, Value::Long(i)) => i.to_string(),
            (_, Value::Float(f)) => f.to_string(),
            (_, Value::Double(f)) => f.to_string(),
            (_, v) => v.as_int().to_string(),
        };
    }
    let cls = vm.class_of(o).map(|c| vm.class_name(c).replace('/', ".")).unwrap_or_default();
    format!("{cls}@{:x}", o.0)
}

pub(crate) fn install(vm: &Vm) {
    install_object_and_class(vm);
    install_strings(vm);
    install_boxes(vm);
    install_system(vm);
    install_throwables(vm);
}

fn install_object_and_class(vm: &Vm) {
    vm.define("java/lang/Object")
        .method("<init>", "()V", |_| void())
        .method("hashCode", "()I", |c| int(c.this.0 as i32))
        .method("equals", "(Ljava/lang/Object;)Z", |c| boolean(c.vm.equals(Some(c.this), c.obj(0))))
        .method("toString", "()Ljava/lang/String;", |c| {
            let t = text_of(c.vm, c.this);
            string(c, &t)
        })
        .method("getClass", "()Ljava/lang/Class;", |c| object(c.vm.class_of(c.this).map(|k| c.vm.class_object(k))));
    for i in [
        "java/lang/Runnable",
        "java/lang/CharSequence",
        "java/lang/Comparable",
        "java/lang/Iterable",
        "java/io/Serializable",
    ] {
        vm.define(i).interface();
    }
    vm.define("java/lang/ClassLoader")
        .method("loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", |c| load_class(c, 0))
        .method("loadClass", "(Ljava/lang/String;Z)Ljava/lang/Class;", |c| load_class(c, 0));
    vm.define("java/lang/Class")
        .static_method("forName", "(Ljava/lang/String;)Ljava/lang/Class;", |c| load_class(c, 0))
        .static_method("forName", "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;", |c| load_class(c, 0))
        .method("getName", "()Ljava/lang/String;", |c| {
            let n = mirror_name(c).replace('/', ".");
            string(c, &n)
        })
        .method("getSimpleName", "()Ljava/lang/String;", |c| {
            let n = mirror_name(c);
            string(c, n.rsplit(['/', '$']).next().unwrap_or_default())
        })
        .method("getClassLoader", "()Ljava/lang/ClassLoader;", |c| object(Some(class_loader(c.vm))))
        .method("isInstance", "(Ljava/lang/Object;)Z", |c| {
            let k = c.vm.class_of_mirror(c.this);
            let o = c.obj(0).and_then(|o| c.vm.class_of(o));
            boolean(matches!((k, o), (Some(k), Some(o)) if c.vm.is_assignable(o, k)))
        })
        .method("getSuperclass", "()Ljava/lang/Class;", |c| {
            let s = c.vm.class_of_mirror(c.this).and_then(|k| c.vm.superclass(k));
            object(s.map(|s| c.vm.class_object(s)))
        })
        .method("toString", "()Ljava/lang/String;", |c| {
            let n = mirror_name(c).replace('/', ".");
            string(c, &format!("class {n}"))
        });
}

fn mirror_name(c: &Call) -> String {
    c.vm.class_of_mirror(c.this).map(|k| c.vm.class_name(k)).unwrap_or_default()
}

/// The single application class loader.
pub(crate) fn class_loader(vm: &Vm) -> Obj {
    vm.singleton("classloader", |vm| {
        let k = vm.find_class("java/lang/ClassLoader").unwrap_or(crate::value::ClassId(0));
        vm.alloc_object(k)
    })
}

fn load_class(c: &mut Call, arg: usize) -> JResult {
    let Some(name) = c.string(arg) else {
        return Err(c.vm.throw_new("java/lang/NullPointerException", "class name"));
    };
    let k = c.vm.find_class(&name)?;
    object(Some(c.vm.class_object(k)))
}

fn install_strings(vm: &Vm) {
    vm.define("java/lang/String")
        .implements(&["java/lang/CharSequence", "java/lang/Comparable", "java/io/Serializable"])
        .payload(|| Data::Str(Vec::new()))
        .method("<init>", "()V", |_| void())
        .method("<init>", "(Ljava/lang/String;)V", |c| {
            let u = c.obj(0).and_then(|o| c.vm.string_units(o)).unwrap_or_default();
            c.vm.with_data(c.this, |d| *d = Data::Str(u));
            void()
        })
        .method("<init>", "([B)V", |c| {
            let bytes = match c.obj(0).and_then(|o| c.vm.data(o)) {
                Some(Data::Prim(_, b)) => b,
                _ => Vec::new(),
            };
            let u = mutf8::units(&String::from_utf8_lossy(&bytes));
            c.vm.with_data(c.this, |d| *d = Data::Str(u));
            void()
        })
        .method("length", "()I", |c| int(str_of(c).len() as i32))
        .method("isEmpty", "()Z", |c| boolean(str_of(c).is_empty()))
        .method("charAt", "(I)C", |c| {
            let u = str_of(c);
            match u.get(c.int(0) as usize) {
                Some(ch) if c.int(0) >= 0 => int(*ch as i32),
                _ => Err(c.vm.throw_new("java/lang/StringIndexOutOfBoundsException", &c.int(0).to_string())),
            }
        })
        .method("hashCode", "()I", |c| int(java_hash(&str_of(c))))
        .method("equals", "(Ljava/lang/Object;)Z", |c| boolean(c.vm.equals(Some(c.this), c.obj(0))))
        .method("toString", "()Ljava/lang/String;", |c| object(Some(c.this)))
        .method("getBytes", "()[B", |c| {
            let s = mutf8::to_string(&str_of(c));
            object(Some(c.vm.new_byte_array(s.as_bytes())))
        })
        .method("substring", "(I)Ljava/lang/String;", |c| substring(c, c.int(0), i32::MAX))
        .method("substring", "(II)Ljava/lang/String;", |c| substring(c, c.int(0), c.int(1)))
        .method("indexOf", "(Ljava/lang/String;)I", |c| {
            let (h, n) = (mutf8::to_string(&str_of(c)), c.string(0).unwrap_or_default());
            int(h.find(&n).map_or(-1, |b| h[..b].encode_utf16().count() as i32))
        })
        .method("startsWith", "(Ljava/lang/String;)Z", |c| {
            boolean(mutf8::to_string(&str_of(c)).starts_with(&c.string(0).unwrap_or_default()))
        })
        .method("endsWith", "(Ljava/lang/String;)Z", |c| {
            boolean(mutf8::to_string(&str_of(c)).ends_with(&c.string(0).unwrap_or_default()))
        })
        .method("contains", "(Ljava/lang/CharSequence;)Z", |c| {
            boolean(mutf8::to_string(&str_of(c)).contains(&c.obj(0).map(|o| text_of(c.vm, o)).unwrap_or_default()))
        })
        .method("toLowerCase", "()Ljava/lang/String;", |c| {
            let s = mutf8::to_string(&str_of(c)).to_lowercase();
            string(c, &s)
        })
        .method("toUpperCase", "()Ljava/lang/String;", |c| {
            let s = mutf8::to_string(&str_of(c)).to_uppercase();
            string(c, &s)
        })
        .method("trim", "()Ljava/lang/String;", |c| {
            let s = mutf8::to_string(&str_of(c));
            string(c, s.trim())
        })
        .method("concat", "(Ljava/lang/String;)Ljava/lang/String;", |c| {
            let s = mutf8::to_string(&str_of(c)) + &c.string(0).unwrap_or_default();
            string(c, &s)
        })
        .method("compareTo", "(Ljava/lang/String;)I", |c| {
            let (a, b) = (mutf8::to_string(&str_of(c)), c.string(0).unwrap_or_default());
            int(a.cmp(&b) as i32)
        })
        .static_method("valueOf", "(Ljava/lang/Object;)Ljava/lang/String;", |c| {
            let t = display(c, c.args[0]);
            string(c, &t)
        })
        .static_method("valueOf", "(I)Ljava/lang/String;", |c| string(c, &c.int(0).to_string()))
        .static_method("valueOf", "(J)Ljava/lang/String;", |c| string(c, &c.long(0).to_string()))
        .static_method("valueOf", "(Z)Ljava/lang/String;", |c| string(c, &(c.int(0) != 0).to_string()));
    for name in ["java/lang/StringBuilder", "java/lang/StringBuffer"] {
        vm.define(name)
            .implements(&["java/lang/CharSequence"])
            .payload(|| Data::Str(Vec::new()))
            .method("<init>", "()V", |_| void())
            .method("<init>", "(I)V", |_| void())
            .method("<init>", "(Ljava/lang/String;)V", |c| {
                let u = c.obj(0).and_then(|o| c.vm.string_units(o)).unwrap_or_default();
                c.vm.with_data(c.this, |d| *d = Data::Str(u));
                void()
            })
            .method("append", "(Ljava/lang/String;)Ljava/lang/StringBuilder;", |c| {
                let t = display(c, c.args[0]);
                append(c, &mutf8::units(&t))
            })
            .method("append", "(Ljava/lang/Object;)Ljava/lang/StringBuilder;", |c| {
                let t = display(c, c.args[0]);
                append(c, &mutf8::units(&t))
            })
            .method("append", "(I)Ljava/lang/StringBuilder;", |c| append(c, &mutf8::units(&c.int(0).to_string())))
            .method("append", "(J)Ljava/lang/StringBuilder;", |c| append(c, &mutf8::units(&c.long(0).to_string())))
            .method("append", "(C)Ljava/lang/StringBuilder;", |c| append(c, &[c.int(0) as u16]))
            .method("append", "(Z)Ljava/lang/StringBuilder;", |c| {
                append(c, &mutf8::units(&(c.int(0) != 0).to_string()))
            })
            .method("length", "()I", |c| int(str_of(c).len() as i32))
            .method("toString", "()Ljava/lang/String;", |c| object(Some(c.vm.new_string_units(str_of(c)))));
    }
}

fn substring(c: &mut Call, from: i32, to: i32) -> JResult {
    let u = str_of(c);
    let to = to.min(u.len() as i32);
    if from < 0 || from > to {
        return Err(c.vm.throw_new("java/lang/StringIndexOutOfBoundsException", &format!("{from},{to}")));
    }
    object(Some(c.vm.new_string_units(u[from as usize..to as usize].to_vec())))
}

fn install_boxes(vm: &Vm) {
    vm.define("java/lang/Number").implements(&["java/io/Serializable"]);
    let kinds: [(&str, &str, Ty); 8] = [
        ("Boolean", "Z", Ty::Bool),
        ("Byte", "B", Ty::Byte),
        ("Character", "C", Ty::Char),
        ("Short", "S", Ty::Short),
        ("Integer", "I", Ty::Int),
        ("Long", "J", Ty::Long),
        ("Float", "F", Ty::Float),
        ("Double", "D", Ty::Double),
    ];
    for (name, d, ty) in kinds {
        let full = format!("java/lang/{name}");
        let b = vm.define(&full);
        let b = if matches!(ty, Ty::Bool | Ty::Char) { b } else { b.extends("java/lang/Number") };
        b.field("value", d)
            .method("<init>", &format!("({d})V"), |c| {
                c.vm.set_field(c.this, "value", c.args[0]);
                void()
            })
            .static_method("valueOf", &format!("({d})L{full};"), move |c| object(c.vm.box_value(c.args[0], ty)))
            .method("toString", "()Ljava/lang/String;", |c| {
                let t = text_of(c.vm, c.this);
                string(c, &t)
            })
            .method("hashCode", "()I", |c| {
                let v = c.vm.unbox(c.this).unwrap_or(Value::Int(0));
                int(match v {
                    Value::Long(l) => (l ^ (l >> 32)) as i32,
                    Value::Float(f) => f.to_bits() as i32,
                    Value::Double(f) => (f.to_bits() ^ (f.to_bits() >> 32)) as i32,
                    v => v.as_int(),
                })
            })
            .method("equals", "(Ljava/lang/Object;)Z", |c| boolean(c.vm.equals(Some(c.this), c.obj(0))));
    }
    for (getter, d, f) in [("intValue", "I", 0), ("longValue", "J", 1), ("floatValue", "F", 2), ("doubleValue", "D", 3)]
    {
        let sig = format!("(){d}");
        vm.define("java/lang/Number").method(getter, &sig, move |c| {
            let v = c.vm.unbox(c.this).unwrap_or(Value::Int(0));
            Ok(match f {
                0 => Value::Int(match v {
                    Value::Long(l) => l as i32,
                    Value::Float(x) => x as i32,
                    Value::Double(x) => x as i32,
                    v => v.as_int(),
                }),
                1 => Value::Long(match v {
                    Value::Float(x) => x as i64,
                    Value::Double(x) => x as i64,
                    v => v.as_long(),
                }),
                2 => Value::Float(match v {
                    Value::Int(i) => i as f32,
                    Value::Long(i) => i as f32,
                    v => v.as_float(),
                }),
                _ => Value::Double(match v {
                    Value::Int(i) => i as f64,
                    Value::Long(i) => i as f64,
                    v => v.as_double(),
                }),
            })
        });
    }
    vm.define("java/lang/Boolean")
        .method("booleanValue", "()Z", |c| boolean(c.vm.unbox(c.this).is_some_and(|v| v.as_int() != 0)));
    vm.define("java/lang/Character").method("charValue", "()C", |c| int(c.vm.unbox(c.this).map_or(0, |v| v.as_int())));
    vm.define("java/lang/Integer").static_method("parseInt", "(Ljava/lang/String;)I", |c| {
        let s = c.string(0).unwrap_or_default();
        match s.trim().parse::<i32>() {
            Ok(v) => int(v),
            Err(_) => Err(c.vm.throw_new("java/lang/NumberFormatException", &s)),
        }
    });
}

fn install_system(vm: &Vm) {
    vm.define("java/lang/System")
        .static_method("currentTimeMillis", "()J", |_| {
            long(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64))
        })
        .static_method("nanoTime", "()J", |c| long(c.vm.now_ms() as i64 * 1_000_000))
        .static_method("identityHashCode", "(Ljava/lang/Object;)I", |c| int(c.obj(0).map_or(0, |o| o.0 as i32)))
        .static_method("gc", "()V", |_| void())
        .static_method("lineSeparator", "()Ljava/lang/String;", |c| string(c, "\n"))
        .static_method("exit", "(I)V", |c| {
            let code = c.int(0);
            c.cpu.request_stop(Stop::Host(format!("System.exit({code})")));
            void()
        })
        .static_method("getenv", "(Ljava/lang/String;)Ljava/lang/String;", |c| {
            match c.string(0).and_then(|k| std::env::var(k).ok()) {
                Some(v) => string(c, &v),
                None => object(None),
            }
        })
        .static_method("getProperty", "(Ljava/lang/String;)Ljava/lang/String;", |c| property(c, None))
        .static_method("getProperty", "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", |c| {
            property(c, Some(1))
        })
        .static_method("arraycopy", "(Ljava/lang/Object;ILjava/lang/Object;II)V", arraycopy);
    vm.define("java/lang/Thread")
        .implements(&["java/lang/Runnable"])
        .static_method("currentThread", "()Ljava/lang/Thread;", |c| {
            object(Some(thread_object(c.vm, c.cpu.processor_id())))
        })
        .static_method("sleep", "(J)V", |c| {
            std::thread::sleep(std::time::Duration::from_millis(c.long(0).max(0) as u64));
            void()
        })
        .method("<init>", "()V", |_| void())
        .method("getName", "()Ljava/lang/String;", |c| {
            let n = c.vm.get_field(c.this, "name").as_ref().and_then(|n| c.vm.string(n));
            string(c, &n.unwrap_or_else(|| "Thread".into()))
        })
        .method("setName", "(Ljava/lang/String;)V", |c| {
            c.vm.set_field(c.this, "name", c.args[0]);
            void()
        })
        .method("getId", "()J", |c| long(c.vm.get_field(c.this, "id").as_long()))
        .method("getPriority", "()I", |_| int(5))
        .method("setPriority", "(I)V", |_| void())
        .method("isInterrupted", "()Z", |_| boolean(false))
        .method("isAlive", "()Z", |_| boolean(true))
        .method("getContextClassLoader", "()Ljava/lang/ClassLoader;", |c| object(Some(class_loader(c.vm))));
}

/// The `Thread` object of guest thread `pid` (the first one is called "main").
pub(crate) fn thread_object(vm: &Vm, pid: usize) -> Obj {
    if let Some(t) = vm.st().thread_objs.get(&pid) {
        return *t;
    }
    let k = vm.find_class("java/lang/Thread").unwrap_or(crate::value::ClassId(0));
    let t = vm.alloc_object(k);
    let name = if pid == 0 { "main".to_string() } else { format!("Thread-{pid}") };
    let n = vm.new_string(&name);
    vm.set_field(t, "name", Value::Ref(Some(n)));
    vm.set_field(t, "id", Value::Long(pid as i64 + 1));
    vm.st().thread_objs.insert(pid, t);
    t
}

fn property(c: &mut Call, default: Option<usize>) -> JResult {
    let key = c.string(0).unwrap_or_default();
    let p = c.vm.platform();
    let (lang, country) = p.language_country();
    let v = match key.as_str() {
        "os.name" => Some("Linux".to_string()),
        "os.arch" => Some("armv7l".into()),
        "java.vm.name" => Some("Dalvik".into()),
        "file.separator" => Some("/".into()),
        "path.separator" => Some(":".into()),
        "line.separator" => Some("\n".into()),
        "user.dir" => Some(p.files_dir.clone()),
        "user.home" => Some(p.files_dir.clone()),
        "java.io.tmpdir" => Some(p.cache_dir.clone()),
        "user.language" => Some(lang),
        "user.region" => Some(country),
        "user.timezone" => Some(p.timezone.clone()),
        _ => default.and_then(|i| c.string(i)),
    };
    match v {
        Some(v) => string(c, &v),
        None => object(None),
    }
}

fn arraycopy(c: &mut Call) -> JResult {
    let (src, sp, dst, dp, n) = (c.obj(0), c.int(1), c.obj(2), c.int(3), c.int(4));
    let (Some(src), Some(dst)) = (src, dst) else {
        return Err(c.vm.throw_new("java/lang/NullPointerException", "arraycopy"));
    };
    let bad = || c.vm.throw_new("java/lang/ArrayIndexOutOfBoundsException", "arraycopy");
    let (Some(sd), Some(dd)) = (c.vm.data(src), c.vm.data(dst)) else { return Err(bad()) };
    let (sl, dl) = (c.vm.array_len(src).unwrap_or(0), c.vm.array_len(dst).unwrap_or(0));
    if sp < 0 || dp < 0 || n < 0 || (sp + n) as usize > sl || (dp + n) as usize > dl {
        return Err(bad());
    }
    let (sp, dp, n) = (sp as usize, dp as usize, n as usize);
    match (sd, dd) {
        (Data::Prim(t1, a), Data::Prim(t2, _)) if t1 == t2 => {
            let sz = t1.size();
            c.vm.with_data(dst, |d| {
                if let Data::Prim(_, b) = d {
                    b[dp * sz..(dp + n) * sz].copy_from_slice(&a[sp * sz..(sp + n) * sz]);
                }
            });
        }
        (Data::Objs(a), Data::Objs(_)) => {
            c.vm.with_data(dst, |d| {
                if let Data::Objs(b) = d {
                    b[dp..dp + n].copy_from_slice(&a[sp..sp + n]);
                }
            });
        }
        _ => return Err(c.vm.throw_new("java/lang/ArrayStoreException", "arraycopy")),
    }
    void()
}

fn install_throwables(vm: &Vm) {
    vm.define("java/lang/Throwable")
        .implements(&["java/io/Serializable"])
        .field("detailMessage", "Ljava/lang/String;")
        .method("<init>", "()V", |_| void())
        .method("<init>", "(Ljava/lang/String;)V", |c| {
            c.vm.set_field(c.this, "detailMessage", c.args[0]);
            void()
        })
        .method("getMessage", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "detailMessage")))
        .method("getLocalizedMessage", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "detailMessage")))
        .method("getCause", "()Ljava/lang/Throwable;", |_| object(None))
        .method("printStackTrace", "()V", |c| {
            let s = throwable_text(c.vm, c.this);
            c.vm.log(&s);
            void()
        })
        .method("toString", "()Ljava/lang/String;", |c| {
            let s = throwable_text(c.vm, c.this);
            string(c, &s)
        });
    let tree: [(&str, &str); 28] = [
        ("Exception", "Throwable"),
        ("Error", "Throwable"),
        ("RuntimeException", "Exception"),
        ("IllegalArgumentException", "RuntimeException"),
        ("IllegalStateException", "RuntimeException"),
        ("NullPointerException", "RuntimeException"),
        ("ClassCastException", "RuntimeException"),
        ("ArithmeticException", "RuntimeException"),
        ("UnsupportedOperationException", "RuntimeException"),
        ("NegativeArraySizeException", "RuntimeException"),
        ("ArrayStoreException", "RuntimeException"),
        ("IndexOutOfBoundsException", "RuntimeException"),
        ("ArrayIndexOutOfBoundsException", "IndexOutOfBoundsException"),
        ("StringIndexOutOfBoundsException", "IndexOutOfBoundsException"),
        ("NumberFormatException", "IllegalArgumentException"),
        ("InterruptedException", "Exception"),
        ("ClassNotFoundException", "Exception"),
        ("LinkageError", "Error"),
        ("NoClassDefFoundError", "LinkageError"),
        ("IncompatibleClassChangeError", "LinkageError"),
        ("NoSuchMethodError", "IncompatibleClassChangeError"),
        ("NoSuchFieldError", "IncompatibleClassChangeError"),
        ("VirtualMachineError", "Error"),
        ("OutOfMemoryError", "VirtualMachineError"),
        ("StackOverflowError", "VirtualMachineError"),
        ("AssertionError", "Error"),
        ("SecurityException", "RuntimeException"),
        ("CloneNotSupportedException", "Exception"),
    ];
    for (name, sup) in tree {
        vm.define(&format!("java/lang/{name}")).extends(&format!("java/lang/{sup}"));
    }
    vm.define("java/io/IOException").extends("java/lang/Exception");
    vm.define("java/io/FileNotFoundException").extends("java/io/IOException");
}

fn throwable_text(vm: &Vm, e: Obj) -> String {
    let cls = vm.class_of(e).map(|c| vm.class_name(c).replace('/', ".")).unwrap_or_default();
    match vm.exception_message(e) {
        Some(m) => format!("{cls}: {m}"),
        None => cls,
    }
}
