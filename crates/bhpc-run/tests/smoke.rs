//! M1 smoke test on a real APK. Ignored by default: set `BHPC_APK` to a user-owned Blockheads APK and run
//! `BHPC_APK=... cargo test -p bhpc-run --test smoke -- --ignored --nocapture`.
//!
//! After all constructors ran, the host calls into the guest: `objc_getClass("UIApplication")` must find a
//! registered class, `CFStringCreateWithCString` must build a string we can read back, and `NSLog` must
//! reach the host's Android-log sink.

use bhpc_bionic::{lock, LogRecord};
use bhpc_run::apkrun::{InitConfig, Session};
use std::sync::{Arc, Mutex};

const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

fn cstr(sess: &Session, s: &str) -> u32 {
    let p = lock(&sess.st).heap.alloc(s.len() as u32 + 1);
    sess.cpu.mem().write_cstr(p, s).unwrap();
    p
}

fn sym(sess: &Session, name: &str) -> u32 {
    sess.linker.lookup(name).unwrap_or_else(|| panic!("no export {name}"))
}

#[test]
#[ignore]
fn objc_runtime_corefoundation_and_nslog_work_after_init() {
    let apk = std::env::var("BHPC_APK").expect("set BHPC_APK to a Blockheads APK");
    let dir = std::env::temp_dir().join(format!("bhpc-smoke-{}", std::process::id()));
    let mut cfg = InitConfig::new(apk.into());
    cfg.data_dir = Some(dir.clone());
    let log: Arc<Mutex<Vec<LogRecord>>> = Arc::default();
    let sink = log.clone();
    let mut sess = Session::open_with(
        &cfg,
        Box::new(std::io::sink()),
        Some(Box::new(move |r| sink.lock().unwrap().push(r.clone()))),
    )
    .expect("open APK");

    let (rep, failures) = sess.run_constructors(&cfg).expect("run constructors");
    assert!(failures.is_empty(), "constructor failures: {failures:?}");
    assert!(rep.ok(), "{rep:?}");
    assert_eq!(lock(&sess.st).unimplemented_hits.len(), 0, "unimplemented imports hit during init");

    // objc_getClass("UIApplication") != NULL
    let name = cstr(&sess, "UIApplication");
    let get_class = sym(&sess, "objc_getClass");
    let class = sess.cpu.call(get_class, &[name]).expect("objc_getClass").r0;
    assert_ne!(class, 0, "UIApplication is not registered");
    for other in ["NSObject", "NSString", "NSMutableDictionary", "UIView"] {
        let n = cstr(&sess, other);
        assert_ne!(sess.cpu.call(get_class, &[n]).unwrap().r0, 0, "{other}");
    }
    let missing = cstr(&sess, "NoSuchClassAnywhere");
    assert_eq!(sess.cpu.call(get_class, &[missing]).unwrap().r0, 0);

    // CFStringCreateWithCString works and round-trips.
    let text = cstr(&sess, "hello from the host");
    let s =
        sess.cpu.call(sym(&sess, "CFStringCreateWithCString"), &[0, text, UTF8]).expect("CFStringCreateWithCString").r0;
    assert_ne!(s, 0);
    assert_eq!(sess.cpu.call(sym(&sess, "CFStringGetLength"), &[s]).unwrap().r0, 19);
    let buf = lock(&sess.st).heap.alloc(64);
    let ok = sess.cpu.call(sym(&sess, "CFStringGetCString"), &[s, buf, 64, UTF8]).unwrap().r0;
    assert_eq!(ok & 0xff, 1);
    assert_eq!(sess.cpu.mem().read_cstr(buf).unwrap(), "hello from the host");

    // NSLog(@"...") reaches the host log.
    let fmt = cstr(&sess, "bhpc smoke test: NSLog reached the host");
    let nsfmt = sess.cpu.call(sym(&sess, "CFStringCreateWithCString"), &[0, fmt, UTF8]).unwrap().r0;
    sess.cpu.call(sym(&sess, "NSLog"), &[nsfmt]).expect("NSLog");
    let seen = log.lock().unwrap().iter().any(|r| r.msg.contains("bhpc smoke test: NSLog reached the host"));
    assert!(seen, "NSLog output did not reach the host log: {:?}", log.lock().unwrap().last());

    sess.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
