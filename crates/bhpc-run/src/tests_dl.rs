//! `dlopen`/`dlsym` over a snapshot of two test modules, directly and through the guest thunks.
use crate::dl::{bionic_symbol, install, DlTable};
use crate::new_env;
use bhpc_loader::dl::{GLOBAL_HANDLE, HOST_HANDLE, RTLD_DEFAULT, RTLD_NEXT};
use bhpc_loader::testelf::{def, Builder};
use bhpc_loader::Linker;
use std::sync::{Arc, Mutex};

struct Rig {
    cpu: Box<bhpc_cpu::Cpu>,
    st: bhpc_bionic::Shared,
    table: Arc<Mutex<Option<DlTable>>>,
}

fn rig() -> Rig {
    let (mut cpu, st) = new_env(Box::new(std::io::sink()), None).unwrap();
    let mut linker = Linker::new(cpu.mem().clone());
    // libdep.so defines `dep_fn`; libmain.so (soname libmain-real.so) needs it and defines `main_fn`.
    let dep = Builder { syms: vec![def("dep_fn", 0x40)], ..Default::default() }.build().bytes;
    let main = Builder {
        syms: vec![def("main_fn", 0x80)],
        needed: vec!["libdep.so", "libc.so"],
        soname: Some("libmain-real.so"),
        ..Default::default()
    }
    .build()
    .bytes;
    let mut hle = bhpc_bionic::Hle::new(&mut cpu, st.clone());
    linker.load("libdep.so", &dep, &mut hle).unwrap();
    linker.load("libmain.so", &main, &mut hle).unwrap();
    let (st2, host_st) = (st.clone(), st.clone());
    let host: crate::dl::HostResolver = Box::new(move |cpu, name| {
        let _ = &st2;
        (name == "host_fn").then_some(0xABCD).or_else(|| bionic_symbol(cpu, &host_st, name))
    });
    let table = Arc::new(Mutex::new(Some(DlTable::new(&linker, &["libextra.so".to_string()], host))));
    install(&cpu, &st, table.clone());
    Rig { cpu, st, table }
}

fn put(r: &mut Rig, s: &str) -> u32 {
    let p = bhpc_bionic::lock(&r.st).heap.alloc(s.len() as u32 + 1);
    r.cpu.mem().write_bytes(p, s.as_bytes()).unwrap();
    r.cpu.mem().write_bytes(p + s.len() as u32, &[0]).unwrap();
    p
}

fn call(r: &mut Rig, name: &str, args: &[u32]) -> u32 {
    let a = r.cpu.thunk_addr(name).unwrap();
    r.cpu.call(a, args).unwrap().r0
}

#[test]
fn table_semantics() {
    let mut r = rig();
    let mut guard = r.table.lock().unwrap();
    let t = guard.as_mut().unwrap();
    assert_eq!(t.dlopen(None), GLOBAL_HANDLE);
    let dep = t.dlopen(Some("libdep.so"));
    let main = t.dlopen(Some("/system/lib/libmain-real.so"));
    assert_eq!((dep, main), (1, 2));
    assert_eq!(t.dlopen(Some("libextra.so")), HOST_HANDLE);
    assert_eq!(t.dlopen(Some("libOpenSLES.so")), HOST_HANDLE);
    assert_eq!(t.dlopen(Some("libnope.so")), 0);
    assert!(t.take_error().unwrap().contains("libnope.so"));
    assert_eq!(t.take_error(), None);
    // Module handle searches the module and its loaded dependencies.
    assert!(t.dlsym(&mut r.cpu, main, "main_fn").is_some());
    assert!(t.dlsym(&mut r.cpu, main, "dep_fn").is_some());
    assert!(t.dlsym(&mut r.cpu, dep, "main_fn").is_none());
    assert!(t.take_error().unwrap().contains("undefined symbol: main_fn"));
    assert!(t.dlsym(&mut r.cpu, 77, "x").is_none());
    assert!(t.take_error().unwrap().contains("invalid handle"));
    // Global scope: modules first, then the host.
    assert!(t.dlsym(&mut r.cpu, RTLD_DEFAULT, "dep_fn").is_some());
    assert_eq!(t.dlsym(&mut r.cpu, GLOBAL_HANDLE, "host_fn"), Some(0xABCD));
    assert_eq!(t.dlsym(&mut r.cpu, HOST_HANDLE, "host_fn"), Some(0xABCD));
    assert_eq!(t.dlsym(&mut r.cpu, RTLD_NEXT, "host_fn"), Some(0xABCD));
    assert!(t.dlsym(&mut r.cpu, HOST_HANDLE, "memcpy").is_some(), "libc through the bionic resolver");
    assert!(t.dlsym(&mut r.cpu, HOST_HANDLE, "no_such_symbol").is_none());
    t.set_trace(true);
    assert_eq!(t.dlopen(Some("libdep.so")), 1);
    assert_eq!(t.dlsym(&mut r.cpu, HOST_HANDLE, "host_fn"), Some(0xABCD));
}

#[test]
fn bionic_symbol_only_binds_known_names() {
    let r = &mut rig();
    assert!(bionic_symbol(&mut r.cpu, &r.st, "strlen").is_some());
    assert!(bionic_symbol(&mut r.cpu, &r.st, "strlen").is_some(), "second lookup finds the bound thunk");
    assert!(bionic_symbol(&mut r.cpu, &r.st, "environ").is_some(), "data symbol");
    assert!(bionic_symbol(&mut r.cpu, &r.st, "definitely_not_libc").is_none());
    assert!(bhpc_bionic::lock(&r.st).unimplemented_bound.is_empty(), "no trap thunk was created");
}

#[test]
fn guest_calls_reach_the_table() {
    let mut r = rig();
    let (dep, nope, sym, host_sym, bad) = (
        put(&mut r, "libdep.so"),
        put(&mut r, "libnope.so"),
        put(&mut r, "dep_fn"),
        put(&mut r, "host_fn"),
        put(&mut r, "x"),
    );
    assert_eq!(call(&mut r, "dlopen", &[0, 0]), GLOBAL_HANDLE);
    let h = call(&mut r, "dlopen", &[dep, 2]);
    assert_eq!(h, 1);
    assert_ne!(call(&mut r, "dlsym", &[h, sym]), 0);
    assert_eq!(call(&mut r, "dlerror", &[]), 0, "no pending error");
    assert_eq!(call(&mut r, "dlopen", &[nope, 2]), 0);
    let e = call(&mut r, "dlerror", &[]);
    assert!(r.cpu.mem().read_cstr(e).unwrap().contains("libnope.so"));
    assert_eq!(call(&mut r, "dlerror", &[]), 0, "cleared by reading");
    assert_eq!(call(&mut r, "dlsym", &[HOST_HANDLE, host_sym]), 0xABCD);
    assert_eq!(call(&mut r, "dlsym", &[HOST_HANDLE, bad]), 0);
    let e2 = call(&mut r, "dlerror", &[]);
    assert_eq!(e2, e, "the message buffer is reused");
    assert_eq!(call(&mut r, "dlclose", &[h]), 0);
    // Bad guest pointers stop the guest instead of panicking.
    let a = r.cpu.thunk_addr("dlopen").unwrap();
    assert!(r.cpu.call(a, &[0xDEAD_0000, 0]).is_err());
    let a = r.cpu.thunk_addr("dlsym").unwrap();
    assert!(r.cpu.call(a, &[1, 0xDEAD_0000]).is_err());
    // Without a table every call answers 0.
    *r.table.lock().unwrap() = None;
    assert_eq!(call(&mut r, "dlopen", &[dep, 2]), 0);
    assert_eq!(call(&mut r, "dlerror", &[]), 0);
}
