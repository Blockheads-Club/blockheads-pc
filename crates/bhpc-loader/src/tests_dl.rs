use crate::dl::*;
use crate::testelf::*;
use crate::*;

fn none() -> impl FnMut(&str) -> Option<u32> {
    |_| None
}

/// Loads libleaf (leaf_fn, sized/unsized/thumb symbols), libmid (needs leaf and a host lib), libapp.
fn world() -> Linker {
    let mut l = Linker::new(Arc::new(GuestMemory::new().unwrap()));
    let mut src = |n: &str| {
        let b = match n {
            "libleaf.so" => Builder {
                soname: Some("libleaf.so"),
                syms: vec![
                    def_sized("leaf_fn", 0x100, 0x20),
                    def("alias_b", 0x200),
                    def("alias_a", 0x200),
                    def_sized("thumb_fn", 0x301, 0x10),
                    def("shared", 0x400),
                ],
                ..Default::default()
            },
            "libmid.so" => Builder {
                needed: vec!["libleaf.so", "libc.so", "libmid.so", "libghost.so"],
                syms: vec![def("mid_fn", 0x10), def("shared", 0x20)],
                ..Default::default()
            },
            "libapp.so" => Builder { needed: vec!["libmid.so"], syms: vec![def("app_fn", 0x10)], ..Default::default() },
            _ => return None,
        };
        Some(b.build().bytes)
    };
    l.load_all(&["libapp.so"], &mut src, &mut none()).unwrap();
    l
}

#[test]
fn dlopen_finds_modules_host_libs_and_reports_errors() {
    let mut l = world();
    assert_eq!(l.dlopen(None), GLOBAL_HANDLE);
    let leaf = l.dlopen(Some("libleaf.so"));
    assert_eq!(leaf, 1);
    assert_eq!(l.dlopen(Some("/system/lib/libmid.so")), 2, "path basename");
    assert_eq!(l.dlopen(Some("libc.so")), HOST_HANDLE);
    assert_eq!(l.dlerror(), None);
    assert_eq!(l.dlopen(Some("libnope.so")), 0);
    assert_eq!(l.dlerror().unwrap(), "dlopen failed: library \"libnope.so\" not found");
    assert_eq!(l.dlerror(), None, "reading clears");
}

#[test]
fn dlsym_handles() {
    let mut l = world();
    let leaf = l.module("libleaf.so").unwrap().bias;
    let mid = l.module("libmid.so").unwrap().bias;
    let mut host = |n: &str| (n == "malloc").then_some(0xAAAA);
    assert_eq!(l.dlsym(RTLD_DEFAULT, "leaf_fn", &mut host), Some(leaf + 0x100));
    assert_eq!(l.dlsym(GLOBAL_HANDLE, "malloc", &mut host), Some(0xAAAA));
    assert_eq!(l.dlsym(HOST_HANDLE, "malloc", &mut host), Some(0xAAAA));
    assert_eq!(l.dlsym(HOST_HANDLE, "free", &mut host), None);
    assert_eq!(l.dlerror().unwrap(), "dlsym failed: undefined symbol: free");
    // A module handle searches the module and its loaded dependencies (cycle and missing libs tolerated).
    let h_mid = l.dlopen(Some("libmid.so"));
    assert_eq!(l.dlsym(h_mid, "mid_fn", &mut host), Some(mid + 0x10));
    assert_eq!(l.dlsym(h_mid, "leaf_fn", &mut host), Some(leaf + 0x100), "through DT_NEEDED");
    assert_eq!(l.dlsym(h_mid, "app_fn", &mut host), None, "dependents are not searched");
    let h_leaf = l.dlopen(Some("libleaf.so"));
    assert_eq!(l.dlsym(h_leaf, "mid_fn", &mut host), None);
    assert_eq!(l.dlsym(99, "x", &mut host), None);
    assert_eq!(l.dlerror().unwrap(), "dlsym failed: invalid handle 0x63");
    assert_eq!(l.dlsym(RTLD_NEXT, "x", &mut host), None);
}

#[test]
fn dlsym_next_searches_after_the_caller() {
    let mut l = world();
    let leaf = l.module("libleaf.so").unwrap().clone();
    let mid = l.module("libmid.so").unwrap().clone();
    // Both define `shared`; from inside leaf the next definition is mid's, from mid there is none.
    assert_eq!(l.dlsym_from(leaf.start + 4, "shared"), Some(mid.bias + 0x20));
    assert_eq!(l.dlsym_from(mid.start + 4, "shared"), None);
    assert!(l.dlerror().unwrap().contains("shared"));
    assert_eq!(l.dlsym_from(0x10, "shared"), None, "caller outside every module");
}

#[test]
fn dladdr_names_the_nearest_symbol() {
    let l = world();
    let leaf = l.module("libleaf.so").unwrap().clone();
    let info = l.dladdr(leaf.bias + 0x108).unwrap();
    assert_eq!(info.module, "libleaf.so");
    assert_eq!(info.base, leaf.start);
    assert_eq!(info.symbol, Some(("leaf_fn".to_string(), leaf.bias + 0x100)));
    // Aliases: first in name order. Unsized symbols cover everything up to the next one.
    assert_eq!(l.dladdr(leaf.bias + 0x210).unwrap().symbol, Some(("alias_a".to_string(), leaf.bias + 0x200)));
    // Past a sized symbol's end: no symbol, but the module is still reported.
    let gap = l.dladdr(leaf.bias + 0x150).unwrap();
    assert_eq!((gap.module.as_str(), gap.symbol), ("libleaf.so", None));
    // Thumb symbols keep bit 0 in the reported address.
    assert_eq!(l.dladdr(leaf.bias + 0x304).unwrap().symbol, Some(("thumb_fn".to_string(), leaf.bias + 0x301)));
    // Before the first symbol of a module without any earlier one.
    let app = l.module("libapp.so").unwrap();
    assert_eq!(l.dladdr(app.bias + 8).unwrap().symbol, None);
    assert_eq!(l.dladdr(0x10), None);
}
