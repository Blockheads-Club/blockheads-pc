//! `java.util`.

use super::{boolean, int, object, string, void};
use crate::value::{Obj, Value};
use crate::vm::{Call, Data, Vm};

fn with_list<R>(c: &Call, f: impl FnOnce(&mut Vec<Option<Obj>>) -> R) -> Option<R> {
    c.vm.with_data(c.this, |d| match d {
        Data::List(v) => Some(f(v)),
        _ => None,
    })
    .flatten()
}

fn with_map<R>(c: &Call, f: impl FnOnce(&mut Vec<(Option<Obj>, Option<Obj>)>) -> R) -> Option<R> {
    c.vm.with_data(c.this, |d| match d {
        Data::Map(v) => Some(f(v)),
        _ => None,
    })
    .flatten()
}

fn index_error(c: &Call, i: i32, n: usize) -> crate::vm::Throw {
    c.vm.throw_new("java/lang/IndexOutOfBoundsException", &format!("Invalid index {i}, size is {n}"))
}

/// A new `ArrayList` holding `items`.
pub(crate) fn new_list(vm: &Vm, class: &str, items: Vec<Option<Obj>>) -> Obj {
    vm.new_data_object(class, Data::List(items))
}

fn install_list(vm: &Vm, name: &str) {
    vm.define(name)
        .implements(&["java/util/List", "java/util/Collection", "java/lang/Iterable"])
        .payload(|| Data::List(Vec::new()))
        .method("<init>", "()V", |_| void())
        .method("<init>", "(I)V", |_| void())
        .method("size", "()I", |c| int(with_list(c, |v| v.len()).unwrap_or(0) as i32))
        .method("isEmpty", "()Z", |c| boolean(with_list(c, |v| v.is_empty()).unwrap_or(true)))
        .method("clear", "()V", |c| {
            with_list(c, |v| v.clear());
            void()
        })
        .method("add", "(Ljava/lang/Object;)Z", |c| {
            let o = c.obj(0);
            with_list(c, |v| v.push(o));
            boolean(true)
        })
        .method("add", "(ILjava/lang/Object;)V", |c| {
            let (i, o) = (c.int(0), c.obj(1));
            match with_list(c, |v| {
                if i >= 0 && i as usize <= v.len() {
                    v.insert(i as usize, o);
                    true
                } else {
                    false
                }
            }) {
                Some(true) => void(),
                _ => Err(index_error(c, i, with_list(c, |v| v.len()).unwrap_or(0))),
            }
        })
        .method("get", "(I)Ljava/lang/Object;", |c| {
            let i = c.int(0);
            match with_list(c, |v| if i >= 0 { v.get(i as usize).copied() } else { None }) {
                Some(Some(o)) => object(o),
                _ => Err(index_error(c, i, with_list(c, |v| v.len()).unwrap_or(0))),
            }
        })
        .method("set", "(ILjava/lang/Object;)Ljava/lang/Object;", |c| {
            let (i, o) = (c.int(0), c.obj(1));
            match with_list(c, |v| {
                if i >= 0 && (i as usize) < v.len() {
                    Some(std::mem::replace(&mut v[i as usize], o))
                } else {
                    None
                }
            }) {
                Some(Some(old)) => object(old),
                _ => Err(index_error(c, i, with_list(c, |v| v.len()).unwrap_or(0))),
            }
        })
        .method("remove", "(I)Ljava/lang/Object;", |c| {
            let i = c.int(0);
            match with_list(c, |v| if i >= 0 && (i as usize) < v.len() { Some(v.remove(i as usize)) } else { None }) {
                Some(Some(old)) => object(old),
                _ => Err(index_error(c, i, with_list(c, |v| v.len()).unwrap_or(0))),
            }
        })
        .method("indexOf", "(Ljava/lang/Object;)I", |c| {
            let items = with_list(c, |v| v.clone()).unwrap_or_default();
            int(items.iter().position(|x| c.vm.equals(*x, c.obj(0))).map_or(-1, |p| p as i32))
        })
        .method("contains", "(Ljava/lang/Object;)Z", |c| {
            let items = with_list(c, |v| v.clone()).unwrap_or_default();
            boolean(items.iter().any(|x| c.vm.equals(*x, c.obj(0))))
        })
        .method("toArray", "()[Ljava/lang/Object;", |c| {
            let items = with_list(c, |v| v.clone()).unwrap_or_default();
            let a = c.vm.new_object_array("java/lang/Object", 0);
            c.vm.with_data(a, |d| *d = Data::Objs(items));
            object(Some(a))
        })
        .method("iterator", "()Ljava/util/Iterator;", |c| {
            let items = with_list(c, |v| v.clone()).unwrap_or_default();
            object(Some(new_list(c.vm, "java/util/ListIterator$Impl", items)))
        });
}

fn install_map(vm: &Vm, name: &str) {
    vm.define(name)
        .implements(&["java/util/Map"])
        .payload(|| Data::Map(Vec::new()))
        .method("<init>", "()V", |_| void())
        .method("<init>", "(I)V", |_| void())
        .method("size", "()I", |c| int(with_map(c, |m| m.len()).unwrap_or(0) as i32))
        .method("isEmpty", "()Z", |c| boolean(with_map(c, |m| m.is_empty()).unwrap_or(true)))
        .method("clear", "()V", |c| {
            with_map(c, |m| m.clear());
            void()
        })
        .method("put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", |c| {
            let (k, v) = (c.obj(0), c.obj(1));
            let slot = find_key(c, k);
            let old = with_map(c, |m| match slot {
                Some(i) => std::mem::replace(&mut m[i].1, v),
                None => {
                    m.push((k, v));
                    None
                }
            });
            object(old.flatten())
        })
        .method("get", "(Ljava/lang/Object;)Ljava/lang/Object;", |c| {
            let slot = find_key(c, c.obj(0));
            object(slot.and_then(|i| with_map(c, |m| m[i].1)).flatten())
        })
        .method("containsKey", "(Ljava/lang/Object;)Z", |c| boolean(find_key(c, c.obj(0)).is_some()))
        .method("remove", "(Ljava/lang/Object;)Ljava/lang/Object;", |c| {
            let slot = find_key(c, c.obj(0));
            object(slot.and_then(|i| with_map(c, |m| m.remove(i).1)).flatten())
        })
        .method("keySet", "()Ljava/util/Set;", |c| {
            let keys = with_map(c, |m| m.iter().map(|e| e.0).collect()).unwrap_or_default();
            object(Some(new_list(c.vm, "java/util/LinkedHashSet", keys)))
        })
        .method("values", "()Ljava/util/Collection;", |c| {
            let vals = with_map(c, |m| m.iter().map(|e| e.1).collect()).unwrap_or_default();
            object(Some(new_list(c.vm, "java/util/ArrayList", vals)))
        });
}

fn find_key(c: &Call, k: Option<Obj>) -> Option<usize> {
    let keys = with_map(c, |m| m.iter().map(|e| e.0).collect::<Vec<_>>())?;
    keys.iter().position(|x| c.vm.equals(*x, k))
}

pub(crate) fn install(vm: &Vm) {
    for i in ["java/util/Collection", "java/util/List", "java/util/Set", "java/util/Map", "java/util/Iterator"] {
        vm.define(i).interface();
    }
    install_list(vm, "java/util/ArrayList");
    install_list(vm, "java/util/LinkedList");
    install_list(vm, "java/util/Vector");
    install_list(vm, "java/util/LinkedHashSet");
    install_map(vm, "java/util/HashMap");
    install_map(vm, "java/util/LinkedHashMap");
    install_map(vm, "java/util/TreeMap");
    vm.define("java/util/HashSet").extends("java/util/LinkedHashSet");
    vm.define("java/util/LinkedHashSet").implements(&["java/util/Set"]);
    vm.define("java/util/ArrayList").method("addAll", "(Ljava/util/Collection;)Z", |c| {
        let other = c.obj(0).and_then(|o| c.vm.data(o));
        let items = match other {
            Some(Data::List(v)) => v,
            _ => Vec::new(),
        };
        let changed = !items.is_empty();
        with_list(c, |v| v.extend(items));
        boolean(changed)
    });
    // The iterator of the fake lists is a list that is consumed from the front.
    vm.define("java/util/ListIterator$Impl")
        .implements(&["java/util/Iterator"])
        .method("hasNext", "()Z", |c| boolean(with_list(c, |v| !v.is_empty()).unwrap_or(false)))
        .method("next", "()Ljava/lang/Object;", |c| match with_list(c, |v| (!v.is_empty()).then(|| v.remove(0))) {
            Some(Some(o)) => object(o),
            _ => Err(c.vm.throw_new("java/util/NoSuchElementException", "")),
        });
    vm.define("java/util/NoSuchElementException").extends("java/lang/RuntimeException");

    vm.define("java/util/Locale")
        .field("language", "Ljava/lang/String;")
        .field("country", "Ljava/lang/String;")
        .static_method("getDefault", "()Ljava/util/Locale;", |c| object(Some(default_locale(c.vm))))
        .method("getLanguage", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "language")))
        .method("getCountry", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "country")))
        .method("toString", "()Ljava/lang/String;", |c| {
            let f = |n: &str| c.vm.get_field(c.this, n).as_ref().and_then(|s| c.vm.string(s)).unwrap_or_default();
            let (l, k) = (f("language"), f("country"));
            string(c, &if k.is_empty() { l } else { format!("{l}_{k}") })
        });
    vm.define("java/util/TimeZone")
        .static_method("getDefault", "()Ljava/util/TimeZone;", |c| {
            let z = c.vm.singleton("timezone", |vm| {
                let k = vm.find_class("java/util/TimeZone").unwrap_or(crate::value::ClassId(0));
                let z = vm.alloc_object(k);
                let id = vm.new_string(&vm.platform().timezone);
                vm.set_field(z, "id", Value::Ref(Some(id)));
                z
            });
            object(Some(z))
        })
        .method("getID", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "id")))
        .method("getRawOffset", "()I", |_| int(0));
    vm.define("java/util/UUID")
        .field("text", "Ljava/lang/String;")
        .static_method("randomUUID", "()Ljava/util/UUID;", |c| {
            let u = c.vm.alloc_object(c.class);
            let t = c.vm.new_string(&random_uuid(c.vm));
            c.vm.set_field(u, "text", Value::Ref(Some(t)));
            object(Some(u))
        })
        .method("toString", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "text")));
}

fn default_locale(vm: &Vm) -> Obj {
    vm.singleton("locale", |vm| {
        let k = vm.find_class("java/util/Locale").unwrap_or(crate::value::ClassId(0));
        let l = vm.alloc_object(k);
        let (lang, country) = vm.platform().language_country();
        let (a, b) = (vm.new_string(&lang), vm.new_string(&country));
        vm.set_field(l, "language", Value::Ref(Some(a)));
        vm.set_field(l, "country", Value::Ref(Some(b)));
        l
    })
}

/// A version-4 style UUID derived from the VM clock and a counter (not cryptographic).
fn random_uuid(vm: &Vm) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let mut x = N.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed) ^ vm.now_ms();
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (a, b) = (next(), next());
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16 & 0xfff,
        (b >> 48) as u16 & 0x3fff | 0x8000,
        b & 0xffff_ffff_ffff
    )
}
