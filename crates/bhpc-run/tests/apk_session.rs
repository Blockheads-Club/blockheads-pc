//! `bhpc-run --apk` machinery on synthetic APKs built from our own tiny ELF images (no game bytes).

use bhpc_bionic::lock;
use bhpc_loader::apk::Release;
use bhpc_loader::testelf::{build_zip, def, undef, Builder, Rel};
use bhpc_loader::R_ARM_GLOB_DAT;
use bhpc_run::apkrun::{init_only, print_report, InitConfig, Session};
use bhpc_run::RunError;
use std::path::PathBuf;

const BX_LR: u32 = 0xE12F_FF1E;
const UDF: u32 = 0xE7F0_00F0;
const SPIN: u32 = 0xEAFF_FFFE; // b .

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "bhpc-apk-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A library whose scratch words are `code`; `configure` receives the builder and the vaddr of `code[0]`.
fn lib(code: &[u32], configure: impl FnOnce(&mut Builder, u32)) -> Vec<u8> {
    let mut b = Builder { scratch: code.to_vec(), ..Default::default() };
    let at = b.build().scratch_vaddr;
    configure(&mut b, at);
    b.build().bytes
}

/// `ctor(): import()` for the import symbol with 1-based index `sym`.
fn calls_import(sym: u32) -> (Vec<u32>, impl Fn(&mut Builder, u32)) {
    let code = vec![0xE52D_E004, 0xE59F_C004, 0xE12F_FF3C, 0xE49D_F004, 0];
    (code, move |b: &mut Builder, at: u32| {
        b.rels = vec![Rel { off: at + 16, kind: R_ARM_GLOB_DAT, sym }];
    })
}

fn apk_file(dir: &Tmp, libs: &[(&str, Vec<u8>)]) -> PathBuf {
    let names: Vec<String> = libs.iter().map(|(n, _)| format!("lib/armeabi-v7a/{n}")).collect();
    let entries: Vec<(&str, &[u8], bool)> =
        names.iter().zip(libs).map(|(n, (_, bytes))| (n.as_str(), bytes.as_slice(), false)).collect();
    let path = dir.0.join("game.apk");
    std::fs::write(&path, build_zip(&entries)).unwrap();
    path
}

fn config(apk: PathBuf) -> InitConfig {
    let mut cfg = InitConfig::new(apk);
    cfg.force = true;
    cfg
}

fn report_text(r: &bhpc_run::apkrun::InitReport, top: usize) -> String {
    let mut out = Vec::new();
    print_report(r, top, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// libv with a `libv_init` (marks word 6) and a `JNI_OnLoad` (stores its first argument in word 7).
fn libv() -> Vec<u8> {
    lib(&[0xE3A0_1001, 0xE58F_100C, 0xE3A0_0000, BX_LR, 0xE58F_0004, BX_LR, 0, 0], |b, at| {
        b.syms = vec![def("libv_init", at), def("JNI_OnLoad", at + 16)]
    })
}

#[test]
fn constructors_hooks_and_report_on_a_healthy_apk() {
    let dir = Tmp::new("ok");
    let (code, cfgr) = calls_import(2);
    let app = lib(&code, |b, at| {
        b.needed = vec!["libv.so"];
        b.init = Some(at);
        b.syms = vec![def("anchor", at), undef("getpid")];
        cfgr(b, at);
    });
    let apk = apk_file(&dir, &[("libApplication.so", app), ("libv.so", libv())]);
    let mut cfg = config(apk.clone());
    cfg.data_dir = Some(dir.0.join("data"));
    cfg.package = "org.example.app".into();
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert!(rep.ok(), "{rep:?}");
    assert_eq!((rep.modules, rep.constructors_ran, rep.constructors_total), (2, 1, 1));
    assert_eq!(rep.release, None, "an unknown build is accepted with --force");
    assert_eq!(
        rep.per_module.iter().map(|(n, c)| (n.as_str(), *c)).collect::<Vec<_>>(),
        [("libv.so", 0), ("libApplication.so", 1)]
    );
    assert_eq!(rep.histogram.iter().find(|(n, _)| n == "getpid").map(|(_, c)| *c), Some(1));
    assert!(rep.unimplemented_hits.is_empty());
    assert_eq!((rep.threads_created, rep.threads_alive), (0, 0));
    assert!(dir.0.join("data/data/files").is_dir() && dir.0.join("data/sdcard/Download").is_dir());
    let text = report_text(&rep, 25);
    for needle in [
        "build unknown",
        "constructors: 1/1 returned",
        "libApplication.so",
        "thunk calls",
        "getpid",
        "guest threads: 0 created",
        "jni: 0 classes",
        "0 hit during init",
    ] {
        assert!(text.contains(needle), "{needle} in\n{text}");
    }
    assert!(!text.contains("still bound to traps"), "no list when nothing was hit");
}

#[test]
fn the_loader_hooks_run_in_order_with_the_fake_java_vm() {
    let dir = Tmp::new("hooks");
    let app = lib(&[BX_LR], |b, at| {
        b.needed = vec!["libv.so"];
        b.init = Some(at);
    });
    let apk = apk_file(&dir, &[("libApplication.so", app), ("libv.so", libv())]);
    let cfg = config(apk);
    let mut sess = Session::open(&cfg, Box::new(std::io::sink())).unwrap();
    let (rep, failures) = sess.run_constructors(&cfg).unwrap();
    assert!(failures.is_empty() && rep.ok());
    let bias = sess.linker.module("libv.so").unwrap().bias;
    let at = 0x1000 + 52 + 32 * 4 - 0x1000; // scratch starts right after the headers
    let _ = at;
    // Word 6 was set by libv_init, word 7 holds the JavaVM pointer JNI_OnLoad received.
    let mem = sess.cpu.mem();
    let base = bias + 52 + 32 * 4;
    assert_eq!(mem.read_u32(base + 24).unwrap(), 1);
    assert_eq!(mem.read_u32(base + 28).unwrap(), sess.jni.vm);
    assert_eq!(sess.host_libs.len(), 0);
    assert_eq!(sess.range.len(), 2);
    sess.shutdown();
}

#[test]
fn failures_come_with_a_stop_reason_registers_and_a_backtrace() {
    let dir = Tmp::new("fail");
    let app = lib(&[UDF], |b, at| b.init = Some(at));
    let cfg = config(apk_file(&dir, &[("libApplication.so", app)]));
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert!(!rep.ok());
    assert_eq!(rep.constructors_ran, 0);
    // The same failure with the constructor log switched on.
    let mut verbose = cfg.clone();
    verbose.verbose = true;
    assert_eq!(init_only(&verbose, Box::new(std::io::sink())).unwrap().failures.len(), 1);
    let f = &rep.failures[0];
    assert!(f.stop.contains("undefined arm instruction"), "{}", f.stop);
    assert!(f.regs.is_some() && !f.backtrace.is_empty());
    let text = report_text(&rep, 5);
    assert!(text.contains("FAILED libApplication.so") && text.contains("regs: r0=") && text.contains("#0"), "{text}");
    // keep_going continues past the failure to the next constructor.
    let bad_v = lib(&[UDF], |b, at| b.init = Some(at));
    let app = lib(&[BX_LR], |b, at| {
        b.needed = vec!["libv.so"];
        b.init = Some(at);
    });
    let mut cfg = config(apk_file(&dir, &[("libApplication.so", app), ("libv.so", bad_v)]));
    cfg.keep_going = true;
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert_eq!((rep.constructors_ran, rep.failures.len(), rep.constructors_total), (1, 1, 2));
    assert_eq!(rep.failures[0].failure.module, "libv.so");
    assert_eq!(rep.failures[0].failure.module, "libv.so");
}

#[test]
fn unimplemented_imports_are_named_and_listed() {
    let dir = Tmp::new("unimpl");
    let (code, cfgr) = calls_import(1);
    let app = lib(&code, |b, at| {
        b.init = Some(at);
        b.syms = vec![undef("no_such_import")];
        cfgr(b, at);
    });
    let cfg = config(apk_file(&dir, &[("libApplication.so", app)]));
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert_eq!(rep.unimplemented_hits, ["no_such_import"]);
    assert!(rep.failures[0].stop.contains("unimplemented import no_such_import"));
    assert!(rep.failures[0].backtrace[0].contains("<host no_such_import>"), "{:?}", rep.failures[0].backtrace);
    let text = report_text(&rep, 10);
    assert!(text.contains("HIT no_such_import") && text.contains("still bound to traps (1)"), "{text}");
    // With top = 0 the long list is suppressed.
    assert!(!report_text(&rep, 0).contains("still bound to traps"));
}

#[test]
fn a_runaway_constructor_is_stopped_by_the_cycle_budget() {
    let dir = Tmp::new("spin");
    let app = lib(&[SPIN], |b, at| b.init = Some(at));
    let apk = apk_file(&dir, &[("libApplication.so", app)]);
    let mut cfg = config(apk.clone());
    cfg.watchdog_cycles = 1_000_000;
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert!(rep.failures[0].failure.to_string().contains("watchdog"), "{}", rep.failures[0].failure);
}

#[test]
fn hook_failures_are_reported_like_constructor_failures() {
    let dir = Tmp::new("hookfail");
    // JNI_OnLoad dies: reported, and nothing after it runs unless keep_going.
    let bad_v = lib(&[UDF, UDF], |b, at| b.syms = vec![def("JNI_OnLoad", at), def("libv_init", at + 4)]);
    let app = lib(&[BX_LR], |b, at| {
        b.needed = vec!["libv.so"];
        b.init = Some(at);
    });
    let apk = apk_file(&dir, &[("libApplication.so", app), ("libv.so", bad_v)]);
    let rep = init_only(&config(apk.clone()), Box::new(std::io::sink())).unwrap();
    assert_eq!(rep.failures.len(), 1);
    assert_eq!(rep.constructors_ran, 0, "libApplication never started");
    let mut cfg = config(apk);
    cfg.keep_going = true;
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert_eq!(rep.failures.len(), 1, "the first failing hook stops that library's hooks");
    assert_eq!(rep.constructors_ran, 1, "keep_going moves on to the next library");
}

#[test]
fn stops_outside_any_module_still_get_a_backtrace() {
    let dir = Tmp::new("wild");
    // ctor: jump to an unmapped address.
    let app = lib(&[0xE3A0_0102, 0xE12F_FF10], |b, at| b.init = Some(at)); // mov r0,#0x80000000; bx r0
    let cfg = config(apk_file(&dir, &[("libApplication.so", app)]));
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    let f = &rep.failures[0];
    assert!(f.backtrace.len() >= 2, "{:?}", f.backtrace);
    assert!(f.backtrace[0].starts_with("0x8000000"), "{:?}", f.backtrace);
}

#[test]
fn tracing_options_are_honoured() {
    let dir = Tmp::new("trace");
    let (code, cfgr) = calls_import(1);
    let app = lib(&code, |b, at| {
        b.init = Some(at);
        b.syms = vec![undef("getpid")];
        cfgr(b, at);
    });
    let mut cfg = config(apk_file(&dir, &[("libApplication.so", app)]));
    let trace = dir.0.join("pcs.txt");
    cfg.pc_trace = Some(trace.clone());
    cfg.verbose = true;
    cfg.trace_fs = true;
    cfg.trace_jni = true;
    cfg.trace_thunks = true;
    let rep = init_only(&cfg, Box::new(std::io::sink())).unwrap();
    assert!(rep.ok());
    assert!(std::fs::read_to_string(&trace).unwrap().lines().count() >= 4);
    // A trace file that cannot be created is an I/O error.
    cfg.pc_trace = Some(dir.0.join("no/such/dir/pcs.txt"));
    assert!(matches!(init_only(&cfg, Box::new(std::io::sink())), Err(RunError::Io(_))));
}

#[test]
fn the_apk_is_mounted_for_the_guest_and_unusable_apks_are_rejected() {
    let dir = Tmp::new("mount");
    let ok = lib(&[BX_LR], |b, at| b.init = Some(at));
    let apk = apk_file(&dir, &[("libApplication.so", ok.clone())]);
    let cfg = config(apk.clone());
    let sess = Session::open(&cfg, Box::new(std::io::sink())).unwrap();
    let guest_path = lock(&sess.st).vfs.config().apk_path();
    let fd = lock(&sess.st).vfs.open(&guest_path, 0).unwrap();
    assert_eq!(lock(&sess.st).vfs.read(fd, 4).unwrap(), b"PK\x03\x04");
    // Not an APK / missing file / unknown build without --force / no libApplication.
    let junk = dir.0.join("junk.apk");
    std::fs::write(&junk, b"not a zip").unwrap();
    assert!(matches!(Session::open(&config(junk), Box::new(std::io::sink())), Err(RunError::Apk(_))));
    assert!(matches!(
        Session::open(&config(dir.0.join("missing.apk")), Box::new(std::io::sink())),
        Err(RunError::Apk(_))
    ));
    let mut strict = InitConfig::new(apk);
    strict.force = false;
    let err = Session::open(&strict, Box::new(std::io::sink())).err().unwrap();
    assert!(err.to_string().contains("unknown Blockheads build"), "{err}");
    let broken = apk_file(&dir, &[("libApplication.so", b"not an elf".to_vec())]);
    assert!(matches!(Session::open(&config(broken), Box::new(std::io::sink())), Err(RunError::Load(_))));
    let empty = apk_file(&dir, &[("libother.so", ok)]);
    assert!(matches!(Session::open(&config(empty), Box::new(std::io::sink())), Err(RunError::Apk(_))));
}

/// A writer that fails once `left` bytes have been written.
struct Limited {
    left: usize,
}

impl std::io::Write for Limited {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.left < b.len() {
            return Err(std::io::Error::other("full"));
        }
        self.left -= b.len();
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_report_prints_every_section_and_survives_write_errors() {
    use bhpc_loader::init::{CallFailure, CtorFailure, CtorKind};
    use bhpc_run::apkrun::FailureReport;
    let dir = Tmp::new("report");
    let ok = lib(&[BX_LR], |b, at| {
        b.init = Some(at);
        b.exidx = true;
    });
    let mut rep = init_only(&config(apk_file(&dir, &[("libApplication.so", ok)])), Box::new(std::io::sink())).unwrap();
    assert!(rep.ok());
    rep.release = Some(Release::V1_7_6);
    rep.failures = vec![FailureReport {
        failure: CtorFailure {
            module: "libX.so".into(),
            kind: CtorKind::Init,
            entry: 0x10,
            failure: CallFailure::Watchdog { pc: 0x10 },
        },
        stop: "stopped".into(),
        backtrace: vec!["0x10 libX.so+0x10".into(), "0x20".into()],
        regs: None,
    }];
    rep.thread_failure = Some("thread boom".into());
    rep.jni.java_calls = vec![("A.b".into(), 3), ("A.c".into(), 1)];
    rep.jni.exception_pending = true;
    rep.unimplemented_hits = vec!["foo".into()];
    rep.unimplemented_bound = (0..20).map(|i| format!("sym{i}")).collect();
    assert!(!rep.ok());
    let text = report_text(&rep, 25);
    for needle in [
        "build 1.7.6",
        "FAILED libX.so",
        "stop: stopped",
        "#1  0x20",
        "a guest thread failed: thread boom",
        "(exception pending)",
        "     3  A.b",
        "HIT foo",
        "sym19",
    ] {
        assert!(text.contains(needle), "{needle} in\n{text}");
    }
    assert!(!text.contains("regs:"), "no registers recorded");
    // Every `?` in the printer: fail the writer after each possible prefix.
    for left in 0..text.len() {
        assert!(print_report(&rep, 25, &mut Limited { left }).is_err(), "prefix {left}");
    }
    assert!(print_report(&rep, 25, &mut Limited { left: text.len() }).is_ok());
}

#[test]
fn run_errors_describe_themselves() {
    let e: RunError = bhpc_loader::apk::ApkError::NotZip.into();
    assert_eq!(e.to_string(), "not a zip/APK file");
    let e: RunError = bhpc_jni::JniError::OutOfMemory.into();
    assert_eq!(e.to_string(), "jni: guest heap exhausted");
    assert_eq!(Release::V1_7_4.version(), "1.7.4");
}
