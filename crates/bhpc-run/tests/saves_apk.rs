//! Saved worlds are listed: the game finds its worlds by listing `Library/Application Support/saves` through
//! libv's `__wrap_opendir`/`__wrap_readdir`, which build their own bionic-layout `DIR` and hand it to the libc
//! `readdir`. Ignored by default: `BHPC_APK=... cargo test -p bhpc-run --test saves_apk -- --ignored --nocapture`.

use bhpc_bionic::lock;
use bhpc_run::apkrun::{InitConfig, Session};

const SAVES: &str = "/data/data/com.noodlecake.blockheads/files/Library/Application Support/saves";

fn sym(sess: &Session, name: &str) -> u32 {
    sess.linker.lookup(name).unwrap_or_else(|| panic!("no export {name}"))
}

#[test]
#[ignore]
fn libv_lists_the_saved_worlds() {
    let apk = std::env::var("BHPC_APK").expect("set BHPC_APK to a Blockheads APK");
    let dir = std::env::temp_dir().join(format!("bhpc-saves-{}", std::process::id()));
    let saves = dir.join("home/Library/Application Support/saves");
    for w in ["0123456789abcdef0123456789abcdef", "fedcba9876543210fedcba9876543210"] {
        std::fs::create_dir_all(saves.join(w).join("world_db")).unwrap();
    }
    let mut cfg = InitConfig::new(apk.into());
    cfg.data_dir = Some(dir.clone());
    let mut sess = Session::open_with(&cfg, Box::new(std::io::sink()), None).expect("open APK");
    let (_, failures) = sess.run_constructors(&cfg).expect("run constructors");
    assert!(failures.is_empty(), "constructor failures: {failures:?}");

    let path = lock(&sess.st).heap.alloc(SAVES.len() as u32 + 1);
    sess.cpu.mem().write_cstr(path, SAVES).unwrap();
    let d = sess.cpu.call(sym(&sess, "__wrap_opendir"), &[path]).expect("opendir").r0;
    assert_ne!(d, 0, "opendir failed");
    let readdir = sym(&sess, "__wrap_readdir");
    let mut names = Vec::new();
    loop {
        let ent = sess.cpu.call(readdir, &[d]).expect("readdir").r0;
        if ent == 0 {
            break;
        }
        let ty = sess.cpu.mem().read_bytes(ent + 18, 1).unwrap()[0];
        names.push((sess.cpu.mem().read_cstr(ent + 19).unwrap(), ty));
    }
    assert_eq!(sess.cpu.call(sym(&sess, "__wrap_closedir"), &[d]).expect("closedir").r0, 0);
    let worlds: Vec<_> = names.iter().filter(|(n, _)| !n.starts_with('.')).cloned().collect();
    assert_eq!(
        worlds,
        [("0123456789abcdef0123456789abcdef".to_string(), 4), ("fedcba9876543210fedcba9876543210".to_string(), 4)],
        "all entries: {names:?}"
    );

    sess.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
