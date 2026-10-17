//! Tests of the data layout and the standard mount table.

use super::tests::{apk, fake_apk};
use crate::error::VfsError;
use crate::fonts::{FALLBACK_FONTS_XML, SYSTEM_FONTS_XML, VENDOR_FALLBACK_FONTS_XML};
use crate::fs::{FileKind, FileSystem, OpenFlags};
use crate::layout::*;
use std::path::Path;

fn storage() -> (tempfile::TempDir, Storage, DataDirs) {
    let d = tempfile::tempdir().unwrap();
    let dirs = DataDirs::new(d.path().join("data"));
    dirs.create_all().unwrap();
    let apk_path = d.path().join("game.apk");
    std::fs::write(&apk_path, fake_apk()).unwrap();
    let cfg = VfsConfig::new(&apk_path, dirs.clone());
    let s = Storage::open(&cfg).unwrap();
    (d, s, dirs)
}

fn read(fs: &dyn FileSystem, p: &str) -> Vec<u8> {
    let mut h = fs.open(p, OpenFlags::read_only()).unwrap();
    let mut b = vec![0u8; h.len().unwrap() as usize];
    let mut got = 0;
    while got < b.len() {
        got += h.read(&mut b[got..]).unwrap();
    }
    b
}

#[test]
fn data_dirs_layout() {
    let d = tempfile::tempdir().unwrap();
    let dirs = DataDirs::new(d.path().join("r"));
    assert_eq!(dirs.home, d.path().join("r/home"));
    assert_eq!(dirs.prefs_file(), d.path().join("r/prefs.json"));
    dirs.create_all().unwrap();
    for p in [
        "home/Documents",
        "home/Library/Caches",
        "home/Library/Preferences",
        "cache",
        "tmp",
        "appdata",
        "sdcard",
        "obb",
        "fonts",
        "logs",
    ] {
        assert!(dirs.root.join(p).is_dir(), "{p}");
    }
    // Idempotent.
    dirs.create_all().unwrap();
    // A file in the way is an error, not a panic.
    std::fs::write(d.path().join("file"), b"x").unwrap();
    let blocked = DataDirs::new(d.path().join("file"));
    assert!(blocked.create_all().is_err());
}

#[test]
fn default_root_by_platform() {
    assert_eq!(DataDirs::root_in(Some("/x".into())), Some(Path::new("/x/blockheads-pc").to_path_buf()));
    assert_eq!(DataDirs::root_in(None), None);
    if let Some(d) = DataDirs::platform_default() {
        assert!(d.root.ends_with("blockheads-pc"));
    }
}

#[test]
fn guest_paths_and_env() {
    let g = GuestPaths::new("com.x");
    assert_eq!(g.data_dir(), "/data/data/com.x");
    assert_eq!(g.files_dir(), "/data/data/com.x/files");
    assert_eq!(g.cache_dir(), "/data/data/com.x/cache");
    assert_eq!(g.tmp_dir(), "/data/data/com.x/tmp");
    assert_eq!(g.code_path(), "/data/app/com.x-1/base.apk");
    assert_eq!(g.obb_dir(), "/sdcard/Android/obb/com.x");
    let env = g.env();
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("HOME"), Some("/data/data/com.x/files"));
    assert_eq!(get("PACKAGE_CODE_PATH"), Some("/data/app/com.x-1/base.apk"));
    assert_eq!(get("OBBDIR"), Some("/sdcard/Android/obb/com.x"));
    for k in [
        "PACKAGE_NAME",
        "PACKAGE_RESOURCE_PATH",
        "TMPDIR",
        "TEMPDIR",
        "CACHEDIR",
        "EXTERNAL_CACHEDIR",
        "ANDROID_DATA_DIRECTORY",
        "ANDROID_EXTERNAL_STORAGE",
        "ANDROID_DOWNLOAD_DIRECTORY",
    ] {
        assert!(get(k).is_some(), "{k}");
    }
}

#[test]
fn system_info_files() {
    let s = SystemInfo { resolv_conf: "nameserver 9.9.9.9\n".into(), ..Default::default() };
    let c = s.cpuinfo();
    assert_eq!(c.matches("processor\t:").count(), 4);
    assert!(c.contains("Features\t: swp half thumb fastmult vfp edsp neon vfpv3 tls"));
    assert!(c.contains("CPU part\t: 0xc09") && c.contains("CPU architecture: 7"));
    assert!(s.meminfo().starts_with("MemTotal:       2097152 kB"));
    let one = SystemInfo { cpus: 1, ..Default::default() };
    assert_eq!(system_files(&one).stat("/sys/devices/system/cpu/online").unwrap().size, 4);
    let fs = system_files(&s);
    assert_eq!(read(&fs, "/sys/devices/system/cpu/present"), b"0-3\n");
    assert_eq!(read(&fs, "/etc/resolv.conf"), b"nameserver 9.9.9.9\n");
    assert!(read(&fs, "/system/etc/hosts").starts_with(b"127.0.0.1 localhost"));
    for p in [SYSTEM_FONTS_XML, FALLBACK_FONTS_XML, VENDOR_FALLBACK_FONTS_XML] {
        assert!(read(&fs, p).starts_with(b"<?xml"), "{p}");
    }
    assert_eq!(read_or(Path::new("/definitely/not/here"), "fb"), "fb");
    assert!(!SystemInfo::default().resolv_conf.is_empty());
}

#[test]
fn standard_mounts_map_android_paths_to_host() {
    let (_d, s, dirs) = storage();
    let v = &s.vfs;
    let w = OpenFlags { write: true, create: true, ..Default::default() };

    // Saves land in home/ whichever alias the guest uses.
    v.open("/data/data/com.noodlecake.blockheads/files/Documents/w.dat", w).unwrap().write(b"save").unwrap();
    assert_eq!(std::fs::read(dirs.home.join("Documents/w.dat")).unwrap(), b"save");
    assert_eq!(read(&**v, "/data/user/0/com.noodlecake.blockheads/files/Documents/w.dat"), b"save");
    // Cache, tmp, appdata, sdcard aliases.
    v.open("/data/data/com.noodlecake.blockheads/cache/c", w).unwrap();
    assert!(dirs.cache.join("c").is_file());
    v.open("/data/local/tmp/t", w).unwrap();
    v.open("/tmp/u", w).unwrap();
    v.open("/data/data/com.noodlecake.blockheads/tmp/v", w).unwrap();
    assert!(dirs.tmp.join("t").is_file() && dirs.tmp.join("u").is_file() && dirs.tmp.join("v").is_file());
    v.open("/data/data/com.noodlecake.blockheads/databases_marker", w).unwrap();
    assert!(dirs.appdata.join("databases_marker").is_file());
    assert_eq!(v.open("/sdcard/Download/x", w).err(), Some(VfsError::NotFound));
    v.mkdir("/sdcard/Download").unwrap();
    v.open("/storage/emulated/0/Download/x", w).unwrap();
    assert!(dirs.external.join("Download/x").is_file());

    // OBB is read-only for the guest.
    std::fs::write(dirs.obb.join("main.1.obb"), b"obb").unwrap();
    assert_eq!(read(&**v, "/sdcard/Android/obb/com.noodlecake.blockheads/main.1.obb"), b"obb");
    assert_eq!(v.open("/mnt/sdcard/Android/obb/com.noodlecake.blockheads/n", w).err(), Some(VfsError::ReadOnly));

    // The APK is a plain file for libv's zzip, byte for byte.
    let code = s.guest.code_path();
    assert_eq!(read(&**v, &code), fake_apk());
    assert_eq!(v.stat(&code).unwrap().size, fake_apk().len() as u64);

    // Apportable's /System tree comes from com.apportable.system in the APK.
    assert_eq!(read(&**v, "/System/icu/icu.dat"), b"ICU");

    // Fonts and synthetic files.
    assert_eq!(read(&**v, "/system/fonts/DroidSansThai.ttf"), b"THAI");
    assert!(read(&**v, "/system/fonts/Roboto-Regular.ttf").len() > 100_000);
    assert!(read(&**v, SYSTEM_FONTS_XML).starts_with(b"<?xml"));
    assert!(read(&**v, "/proc/cpuinfo").starts_with(b"processor"));
    let mut r = v.open("/dev/urandom", OpenFlags::read_only()).unwrap();
    assert_eq!(r.read(&mut [0u8; 16]), Ok(16));

    // Directory structure
    assert!(v
        .read_dir("/data/data/com.noodlecake.blockheads")
        .unwrap()
        .iter()
        .any(|e| e.name == "files" && e.kind == FileKind::Dir));
    assert!(v.read_dir("/system").unwrap().iter().any(|e| e.name == "fonts"));

    // Unmapped paths are ENOENT and remembered.
    assert_eq!(v.stat("/system/lib/libOpenSLES.so"), Err(VfsError::NotFound));
    assert_eq!(v.take_misses(), ["open /sdcard/Download/x", "stat /system/lib/libOpenSLES.so"]);

    // AssetManager shares the archive.
    assert_eq!(s.assets.open("hello.txt").unwrap().buffer(), b"hello world");
}

#[test]
fn storage_open_reports_bad_apk() {
    let d = tempfile::tempdir().unwrap();
    let cfg = VfsConfig::new(d.path().join("missing.apk"), DataDirs::new(d.path()));
    assert_eq!(Storage::open(&cfg).err(), Some(VfsError::NotFound));
    let _ = apk();
}

/// Local smoke test against a real APK: `BHPC_TEST_APK=/path/to.apk cargo test -p bhpc-vfs -- --ignored`.
#[test]
#[ignore = "needs the user's own APK"]
fn real_apk_smoke() {
    let apk = std::env::var("BHPC_TEST_APK").expect("set BHPC_TEST_APK");
    let d = tempfile::tempdir().unwrap();
    let dirs = DataDirs::new(d.path());
    dirs.create_all().unwrap();
    let s = Storage::open(&VfsConfig::new(apk, dirs)).unwrap();
    let n = s.assets.archive().file_count();
    let font = s.assets.open("GameResources/Fonts/BlockheadsFont-Regular.otf").unwrap();
    let thai = read(&*s.vfs, "/system/fonts/DroidSansThai.ttf");
    let icu = s.vfs.stat("/System/icu/icu.dat").unwrap().size;
    eprintln!("entries={n} otf={} thai={} icu={icu}", font.length(), thai.len());
    assert!(font.length() > 0 && !thai.is_empty() && icu > 0);
    let mut dir = s.assets.open_dir("GameResources/Fonts").unwrap();
    assert!(dir.next_file_name().is_some());
}
