//! The game's preferences survive a restart: CoreFoundation reads `Library/Preferences/<app>.plist` back
//! (`CFURLCreateDataAndPropertiesFromResource` + `CFPropertyListCreateWithData` in the XML preferences domain),
//! through libv's `__wrap_open`/`__wrap_fstat`/`__wrap_read`.
//! Ignored by default: `BHPC_APK=... cargo test -p bhpc-run --test prefs_apk -- --ignored --nocapture`.

use bhpc_bionic::lock;
use bhpc_run::apkrun::{InitConfig, Session};

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AppleLanguages</key>
	<array>
		<string>en</string>
	</array>
	<key>GPGSShouldSignIn</key>
	<true/>
	<key>gdprStatus</key>
	<integer>2</integer>
	<key>totalGamePlayTimePassed</key>
	<real>20</real>
</dict>
</plist>
"#;

fn sym(sess: &Session, name: &str) -> u32 {
    sess.linker.lookup(name).unwrap_or_else(|| panic!("no export {name}"))
}

fn call(sess: &mut Session, name: &str, args: &[u32]) -> u32 {
    let f = sym(sess, name);
    sess.cpu.call(f, args).unwrap_or_else(|e| panic!("{name}: {e}")).r0
}

#[test]
#[ignore]
fn corefoundation_reads_the_preferences_back() {
    let apk = std::env::var("BHPC_APK").expect("set BHPC_APK to a Blockheads APK");
    let dir = std::env::temp_dir().join(format!("bhpc-prefs-{}", std::process::id()));
    let prefs = dir.join("home/Library/Preferences");
    std::fs::create_dir_all(&prefs).unwrap();
    std::fs::write(prefs.join("blockheads.plist"), PLIST).unwrap();
    let mut cfg = InitConfig::new(apk.into());
    cfg.data_dir = Some(dir.clone());
    let mut sess = Session::open_with(&cfg, Box::new(std::io::sink()), None).expect("open APK");
    let (_, failures) = sess.run_constructors(&cfg).expect("run constructors");
    assert!(failures.is_empty(), "constructor failures: {failures:?}");

    let buf = lock(&sess.st).heap.alloc(PLIST.len() as u32 + 64);
    sess.cpu.mem().write_bytes(buf, PLIST.as_bytes()).unwrap();
    let data = call(&mut sess, "CFDataCreate", &[0, buf, PLIST.len() as u32]);
    assert_ne!(data, 0);
    let out = buf + PLIST.len() as u32 + 8;
    sess.cpu.mem().write_bytes(out, &[0; 16]).unwrap();
    let plist = call(&mut sess, "CFPropertyListCreateWithData", &[0, data, 0, out, out + 4]);
    let err = sess.cpu.mem().read_u32(out + 4).unwrap();
    let desc = if err != 0 {
        let d = call(&mut sess, "CFCopyDescription", &[err]);
        let p = call(&mut sess, "CFStringGetCStringPtr", &[d, 0x0800_0100]);
        if p != 0 {
            sess.cpu.mem().read_cstr(p).unwrap_or_default()
        } else {
            "?".into()
        }
    } else {
        String::new()
    };
    assert_ne!(plist, 0, "the plist does not parse: {desc}");
    let dict = call(&mut sess, "CFDictionaryGetTypeID", &[]);
    assert_eq!(call(&mut sess, "CFGetTypeID", &[plist]), dict);
    assert_eq!(call(&mut sess, "CFDictionaryGetCount", &[plist]), 4);

    // The same bytes read back from the file.
    let path = "/data/data/com.noodlecake.blockheads/files/Library/Preferences/blockheads.plist";
    let p = lock(&sess.st).heap.alloc(path.len() as u32 + 1);
    sess.cpu.mem().write_cstr(p, path).unwrap();
    let url = call(&mut sess, "CFURLCreateFromFileSystemRepresentation", &[0, p, path.len() as u32, 0]);
    assert_ne!(url, 0);
    sess.cpu.mem().write_bytes(out, &[0; 16]).unwrap();
    let ok = call(&mut sess, "CFURLCreateDataAndPropertiesFromResource", &[0, url, out, 0, 0, out + 4]);
    let read = sess.cpu.mem().read_u32(out).unwrap();
    assert_ne!(ok & 0xff, 0);
    let len = call(&mut sess, "CFDataGetLength", &[read]);
    let bytes = call(&mut sess, "CFDataGetBytePtr", &[read]);
    let got = sess.cpu.mem().read_bytes(bytes, len).unwrap();
    // libv once served this file from its in-memory cache, whose `fstat` says it is empty.
    assert_eq!(String::from_utf8_lossy(&got), PLIST, "the plist reads back whole");

    sess.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
