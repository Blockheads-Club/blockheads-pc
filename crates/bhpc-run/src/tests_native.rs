//! The libbhmod host of the runner against a fake game: real guest code, the real linker for the mod libraries
//! (fixtures/native/*.so, built from fixtures/native/src), real thunks and nested calls. The fake game has a module
//! `libfake.so` with two ARM functions and a class `Calc` (the fake Objective-C runtime of tests_objchook).

use crate::native::{ModSnap, Natives};
use crate::tests_objchook::{Fake, ADD};
use bhpc_loader::{PF_R, PF_X};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BASE: u32 = 0x2010_0000;
/// `push {r4, lr}; add r0, r0, r1; pop {r4, pc}`: `int test_add(int, int)`.
const FUNC: [u32; 3] = [0xE92D_4010, 0xE080_0001, 0xE8BD_8010];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/native").join(name)
}

struct World {
    f: Fake,
    n: Natives,
    obj: u32,
}

impl World {
    fn new() -> World {
        let mut f = Fake::new();
        f.cpu.mem().map(BASE, 0x1000).unwrap();
        for at in [0x80, 0x40] {
            for (i, w) in FUNC.iter().enumerate() {
                f.cpu.mem().write_u32(BASE + at + 4 * i as u32, *w).unwrap();
            }
        }
        let class = f.class("Calc");
        let imp = f.code(&ADD);
        f.method(class, "add:to:", "i12@0:4i8i12", imp);
        let obj = f.object(class);
        let n = Natives::new(f.rt, f.st.clone(), &[]);
        let exports = HashMap::from([("test_add".to_string(), BASE + 0x80)]);
        n.add_snap(ModSnap {
            name: "/data/app/lib/libfake.so".into(),
            base: BASE,
            dynamic: 0,
            exports,
            segments: vec![bhpc_loader::Segment { start: BASE, len: 0x1000, prot: PF_R | PF_X }],
        });
        World { f, n, obj }
    }

    fn call(&mut self, at: u32, a: u32, b: u32) -> u32 {
        self.f.cpu.call(at, &[a, b]).unwrap().r0
    }

    fn results(&self) -> Vec<i32> {
        let at = self.n.symbol("testmod/libtestmod.so", "bhmod_test_results").unwrap();
        (0..16).map(|i| self.f.cpu.mem().read_u32(at + 4 * i).unwrap() as i32).collect()
    }
}

#[test]
fn a_native_mod_hooks_by_name_and_is_undone_on_unload() {
    let mut w = World::new();
    w.n.register(&w.f.cpu);
    let data = Path::new("/tmp/bhpc-native-data");
    w.n.load(&mut w.f.cpu, "testmod", &fixture("libtestmod.so"), data).unwrap();
    assert_eq!(w.n.hooks(), 2);
    // the library's constructors ran before init
    let ctor = w.n.symbol("testmod/libtestmod.so", "bhmod_test_ctor").unwrap();
    assert_eq!(w.f.cpu.mem().read_u32(ctor).unwrap(), 42);
    let r = w.results();
    assert!(r[0] > 0 && r[1] > 0 && r[2] > 0, "{r:?}");
    assert_eq!(r[3..15], [-2, 1, -1, 1, 1, 0, -2, -1, -1, -1, -7, 1], "{r:?}");
    // the inline hook on test_add by symbol, the swizzle, and the hook by address (undone by the mod itself)
    assert_eq!(w.call(BASE + 0x80, 1, 2), 3 + 100);
    assert_eq!(w.f.send(w.obj, "add:to:", &[2, 3]).unwrap().r0, 10);
    assert_eq!(w.call(BASE + 0x40, 1, 2), 3);
    // the mod's log lines (the null message is dropped)
    let logs = w.n.take_logs();
    assert_eq!(logs, [(2, "[testmod] testmod init".to_string())]);
    assert!(w.n.take_logs().is_empty());
    // unload runs the shutdown function and restores everything
    w.n.unload(&mut w.f.cpu, "testmod");
    assert_eq!(w.results()[15], 1);
    assert_eq!(w.n.hooks(), 0);
    assert_eq!(w.call(BASE + 0x80, 1, 2), 3);
    assert_eq!(w.f.send(w.obj, "add:to:", &[2, 3]).unwrap().r0, 5);
    // the next session loads the same library again (already linked) and hooks again
    w.n.load(&mut w.f.cpu, "testmod", &fixture("libtestmod.so"), data).unwrap();
    assert_eq!(w.call(BASE + 0x80, 1, 2), 3 + 100);
    w.n.unload(&mut w.f.cpu, "testmod");
    w.n.unload(&mut w.f.cpu, "unknown");
}

#[test]
fn failing_libraries_are_reported_and_leave_nothing_hooked() {
    let mut w = World::new();
    let data = Path::new("/tmp/bhpc-native-data");
    let err = w.n.load(&mut w.f.cpu, "failmod", &fixture("libfailmod.so"), data).unwrap_err();
    assert!(err.contains("not registered"), "{err}");
    w.n.register(&w.f.cpu);
    // init returns 7 after hooking: the hook is undone
    let err = w.n.load(&mut w.f.cpu, "failmod", &fixture("libfailmod.so"), data).unwrap_err();
    assert!(err.contains("returned 7"), "{err}");
    assert_eq!(w.n.hooks(), 0);
    assert_eq!(w.call(BASE + 0x80, 1, 2), 3);
    let err = w.n.load(&mut w.f.cpu, "noinit", &fixture("libnoinit.so"), data).unwrap_err();
    assert!(err.contains("bhmod_native_init"), "{err}");
    let err = w.n.load(&mut w.f.cpu, "gone", &fixture("libgone.so"), data).unwrap_err();
    assert!(err.contains("libgone.so"), "{err}");
    let junk = std::env::temp_dir().join("bhpc-native-junk.so");
    std::fs::write(&junk, b"not an elf").unwrap();
    assert!(w.n.load(&mut w.f.cpu, "junk", &junk, data).is_err());
    std::fs::remove_file(junk).unwrap();
}

#[test]
fn a_guest_that_stops_in_init_is_reported() {
    let mut w = World::new();
    w.n.register(&w.f.cpu);
    let err = w.n.load(&mut w.f.cpu, "crashmod", &fixture("libcrashmod.so"), Path::new("/tmp")).unwrap_err();
    assert!(err.contains("bhmod_native_init stopped"), "{err}");
}

#[test]
fn the_global_host_is_set_once() {
    let w = World::new();
    w.n.register(&w.f.cpu);
    crate::native::set(w.n.clone());
    assert!(crate::native::get().is_some());
    crate::native::set(w.n.clone());
}

#[test]
fn the_host_reports_memory_it_cannot_use() {
    use bhmod_native::engine::Host;
    let mut w = World::new();
    w.n.register(&w.f.cpu);
    let class = w.f.class("Blank");
    let blank = w.f.method(class, "blank", "v8@0:4", 0);
    {
        let mut host = w.n.host(&mut w.f.cpu);
        // a method without an implementation cannot be swizzled
        assert_eq!(host.objc_set_imp(blank, 0x1234), None);
        // unmapped memory cannot be read or patched
        assert!(host.read(0x10, 4).is_none());
        assert!(host.write(0x10, &[1]).is_err());
        assert!(host.write_code(0x10, &[1]).is_err());
        // an address outside every segment is patched as plain data and restored to read/write
        let at = host.alloc_exec(64).unwrap();
        host.write(at, &[1, 2, 3, 4]).unwrap();
        assert_eq!(host.read(at, 4).unwrap(), [1, 2, 3, 4]);
        assert_eq!(host.modules().len(), 1);
        assert_eq!(host.symbol(Some("libfake.so"), "test_add"), Some(BASE + 0x80));
        assert_eq!(host.symbol(Some("libother.so"), "test_add"), None);
        assert_eq!(host.symbol(None, "nope"), None);
        // the fake runtime knows no class `Nope`
        assert_eq!(host.objc_method("Nope", "x", false), None);
    }
    // the executable arena is exhausted: the next table cannot be made
    let mem = w.f.cpu.mem().clone();
    while mem.map_anywhere(0x400_0000, 4096).is_ok() {}
    while mem.map_anywhere(0x10_0000, 4096).is_ok() {}
    while mem.map_anywhere(0x1000, 4096).is_ok() {}
    let err = w.n.load(&mut w.f.cpu, "late", &fixture("libtestmod.so"), Path::new("/tmp")).unwrap_err();
    assert!(!err.is_empty());
    let mut host = w.n.host(&mut w.f.cpu);
    // arenas of 64 KiB: the first one was taken by `alloc_exec` above, the second cannot be mapped
    assert!(host.alloc_exec(0x20000).is_err() || host.alloc_exec(0x20000).is_err());
}

#[test]
fn log_lines_of_an_unknown_table_are_attributed_to_native() {
    let mut w = World::new();
    w.n.register(&w.f.cpu);
    let log = w.f.cpu.thunk_addr("bhmod:log").unwrap();
    let msg = w.f.cstr("from nowhere");
    w.f.cpu.call(log, &[0xdead, 3, msg]).unwrap();
    assert_eq!(w.n.take_logs(), [(3, "[native] from nowhere".to_string())]);
}

#[test]
fn the_runner_hooks_its_own_functions_and_reads_import_targets() {
    let mut w = World::new();
    w.n.register(&w.f.cpu);
    // a replacement: a host thunk that returns 7
    let thunk =
        w.f.cpu.register_shared_thunk("test:seven", std::sync::Arc::new(|c: &mut bhpc_cpu::Cpu| c.ret_u32(7))).unwrap();
    w.n.add_snap(ModSnap {
        name: "libfake.so".into(),
        base: BASE,
        dynamic: 0,
        exports: HashMap::from([("fn".into(), BASE + 0x40)]),
        segments: vec![],
    });
    let orig = w.n.hook_internal(&mut w.f.cpu, "fn", thunk).unwrap();
    assert_eq!(w.call(BASE + 0x40, 1, 2), 7);
    assert_eq!(w.call(orig, 1, 2), 3, "the trampoline runs the original");
    assert!(w.n.hook_internal(&mut w.f.cpu, "nothing", thunk).is_err());
    w.n.unhook_internal(&mut w.f.cpu);
    assert_eq!(w.call(BASE + 0x40, 1, 2), 3);
    // a module with an import: dynamic section, symbol table, relocation and GOT slot in guest memory
    let mem = w.f.cpu.mem().clone();
    let m = 0x2020_0000;
    mem.map(m, 0x1000).unwrap();
    let w32 = |at: u32, v: u32| mem.write_u32(m + at, v).unwrap();
    mem.write_bytes(m + 0x200, b"\0glBindTexture\0").unwrap();
    w32(0x310, 1); // symbol 1: name at strtab+1
    w32(0x400, 0x600); // relocation: GOT slot at +0x600, R_ARM_JUMP_SLOT against symbol 1
    w32(0x404, 1 << 8 | 22);
    w32(0x600, 0xabcd_0000);
    for (i, (t, v)) in [(5, 0x200), (6, 0x300), (23, 0x400), (2, 8), (0, 0)].into_iter().enumerate() {
        w32(0x100 + 8 * i as u32, t);
        w32(0x104 + 8 * i as u32, v);
    }
    w.n.add_snap(ModSnap {
        name: "/data/libgot.so".into(),
        base: m,
        dynamic: m + 0x100,
        exports: HashMap::new(),
        segments: vec![],
    });
    assert_eq!(w.n.import_target(&mut w.f.cpu, "libgot.so", "glBindTexture"), Some(0xabcd_0000));
    assert_eq!(w.n.import_target(&mut w.f.cpu, "libgot.so", "nothing"), None);
    assert_eq!(w.n.import_target(&mut w.f.cpu, "libnope.so", "glBindTexture"), None);
    assert_eq!(w.n.module_base("libgot.so"), Some(m));
    assert_eq!(w.n.module_base("libnope.so"), None);
}
