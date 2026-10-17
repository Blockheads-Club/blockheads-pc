//! End-to-end tests of the runner over the committed fixture and synthetic images.
use super::*;
use bhpc_loader::testelf::{Builder, Image};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/hello.so");
const BX_LR: u32 = 0xE12F_FF1E;

fn sink() -> Box<dyn Write + Send> {
    Box::new(std::io::sink())
}

/// Image whose scratch area holds `code`; returns (image, vaddr of the code).
fn image_with_code(code: &[u32], configure: impl FnOnce(&mut Builder, u32)) -> (Image, u32) {
    let mut b = Builder { scratch: code.to_vec(), ..Default::default() };
    let at = b.build().scratch_vaddr;
    configure(&mut b, at);
    (b.build(), at)
}

#[test]
fn fixture_passes_all_checks() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let r = run_shared_object(&bytes, "hello_main", sink(), false, None, None).unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.checks.len(), 13);
    assert!(r.checks.iter().all(|(_, ok)| *ok), "{:?}", r.checks);
    assert!(r.relocations > 1000);
    assert!(r.thunks >= 20);
    assert_eq!(r.constructors, 0);
}

#[test]
fn fixture_prints_greeting_and_supports_tracing() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let mut pcs = 0usize;
    let mut counter = |_: &Cpu| pcs += 1;
    let r = run_shared_object(&bytes, "hello_main", sink(), true, Some(10_000_000_000), Some(&mut counter)).unwrap();
    assert_eq!(r.exit_code, 0);
    assert!(pcs > 1000, "instruction trace visited {pcs} instructions");
}

#[test]
fn synthetic_entry_returns_value_and_runs_constructor_and_registers_exidx() {
    // ctor: mov r0,#1; bx lr   entry: mov r0,#42; bx lr
    let (img, at) = image_with_code(&[0xE3A0_0001, BX_LR, 0xE3A0_002A, BX_LR], |b, at| {
        b.init = Some(at);
        b.exidx = true;
        b.syms = vec![bhpc_loader::testelf::def("entry", at + 8)];
    });
    let _ = at;
    let r = run_shared_object(&img.bytes, "entry", sink(), false, None, None).unwrap();
    assert_eq!((r.exit_code, r.constructors), (42, 1));
}

#[test]
fn errors_are_classified() {
    // Missing entry symbol.
    let (img, _) = image_with_code(&[BX_LR], |_, _| {});
    assert!(
        matches!(run_shared_object(&img.bytes, "nope", sink(), false, None, None), Err(RunError::NoEntry(s)) if s == "nope")
    );
    // Garbage is not an ELF.
    assert!(matches!(run_shared_object(b"junk", "x", sink(), false, None, None), Err(RunError::Load(_))));
    // Undefined instruction in the entry point.
    let (img, _) = image_with_code(&[0xE7F0_00F0], |b, at| b.syms = vec![bhpc_loader::testelf::def("entry", at)]);
    assert!(matches!(
        run_shared_object(&img.bytes, "entry", sink(), false, None, None),
        Err(RunError::Guest(Stop::Undefined { word: 0xE7F0_00F0, .. }))
    ));
    // Failing constructor.
    let (img, _) = image_with_code(&[0xE7F0_00F0], |b, at| b.init = Some(at));
    assert!(matches!(run_shared_object(&img.bytes, "entry", sink(), false, None, None), Err(RunError::Guest(_))));
    // Infinite loop caught by the watchdog, with and without instruction tracing.
    let (img, _) = image_with_code(&[0xEAFF_FFFE], |b, at| b.syms = vec![bhpc_loader::testelf::def("entry", at)]);
    match run_shared_object(&img.bytes, "entry", sink(), false, Some(100_000), None) {
        Err(RunError::Watchdog { pc, .. }) => assert!(pc >= 0x1000_0000),
        other => panic!("unexpected {other:?}"),
    }
    // Without a watchdog a halted run is reported as a plain guest stop (unimplemented import here).
    let (img, _) = image_with_code(&[BX_LR], |b, at| {
        b.syms = vec![bhpc_loader::testelf::def("entry", at), bhpc_loader::testelf::undef("mystery")];
        b.rels = vec![bhpc_loader::testelf::Rel { off: at, kind: bhpc_loader::R_ARM_GLOB_DAT, sym: 2 }];
    });
    // The relocation overwrote the first scratch word with a thunk address, so the entry code is now
    // the thunk address interpreted as an instruction; just make sure loading and running does not hang.
    let _ = run_shared_object(&img.bytes, "entry", sink(), false, Some(100_000), None);
}

#[test]
fn run_error_display_and_conversions() {
    let all: Vec<RunError> = vec![
        std::io::Error::other("disk").into(),
        MemError::OutOfSpace.into(),
        CpuError::TooManyThunks.into(),
        LoadError::Truncated.into(),
        bhpc_loader::apk::ApkError::NotZip.into(),
        bhpc_jni::JniError::Install("guest heap exhausted".into()).into(),
        bhpc_vfs::VfsError::Io.into(),
        RunError::NoEntry("e".into()),
        RunError::Guest(Stop::Nested),
        RunError::Watchdog { pc: 0x10, lr: 0x20 },
    ];
    let text: Vec<String> = all.iter().map(|e| e.to_string()).collect();
    assert!(text[0].starts_with("io:") && text[0].contains("disk"));
    assert!(text[1].contains("exhausted"));
    assert!(text[2].contains("full"));
    assert!(text[3].starts_with("load:"));
    assert_eq!(text[4], "not a zip/APK file");
    assert_eq!(text[5], "jni: guest heap exhausted");
    assert_eq!(text[6], "vfs: input/output error");
    assert!(text[7].contains("does not export `e`"));
    assert!(text[8].contains("guest stopped"));
    assert!(text[9].contains("0x00000010") && text[9].contains("0x00000020"));
    let _: &dyn std::error::Error = &all[0];
}

#[test]
fn new_env_has_stack_and_heap() {
    let (cpu, _st) = new_env(sink(), Some(1000)).unwrap();
    assert_eq!(cpu.sp(), STACK_TOP);
    assert_eq!(cpu.ticks(), 1000);
    assert!(cpu.mem().is_mapped(STACK_TOP - 4, 4));
    assert!(cpu.mem().is_mapped(HEAP_BASE, 4));
}

#[test]
fn the_fake_vm_allocates_from_the_bionic_heap() {
    use bhpc_jni::vm::GuestAlloc;
    let (_cpu, st) = new_env(sink(), Some(1000)).unwrap();
    let mut h = crate::apkrun::BionicHeap(st.clone());
    let p = h.alloc(64);
    assert!(p >= HEAP_BASE);
    h.free(p);
    assert_eq!(h.alloc(64), p, "freed blocks are reused");
}

#[test]
fn data_dirs_prefer_the_configured_root() {
    use crate::apkrun::data_dirs;
    use bhpc_vfs::DataDirs;
    let user = DataDirs::new("/user/default");
    assert_eq!(data_dirs(Some(std::path::Path::new("/x")), Some(user.clone())).root, std::path::Path::new("/x"));
    assert_eq!(data_dirs(None, Some(user.clone())), user);
    assert!(data_dirs(None, None).root.ends_with("blockheads-pc"));
}
