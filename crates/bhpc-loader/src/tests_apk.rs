use crate::apk::*;
use crate::testelf::build_zip;
use crate::*;

const LIB: &str = "lib/armeabi-v7a/libApplication.so";

fn sample() -> Vec<u8> {
    let big: Vec<u8> = (0..5000u32).map(|i| (i % 7) as u8).collect();
    build_zip(&[
        ("classes.dex", b"dex", false),
        (LIB, &big, true),
        ("lib/armeabi-v7a/libstored.so", b"stored-bytes", false),
        ("lib/armeabi-v7a/sub/libnested.so", b"x", false),
        ("lib/armeabi-v7a/readme.txt", b"x", false),
        ("lib/x86/libApplication.so", b"x", false),
    ])
}

fn eocd(z: &[u8]) -> usize {
    z.len() - 22
}

fn cd_off(z: &[u8]) -> usize {
    u32::from_le_bytes(z[eocd(z) + 16..eocd(z) + 20].try_into().unwrap()) as usize
}

fn patch(mut z: Vec<u8>, at: usize, bytes: &[u8]) -> Vec<u8> {
    z[at..at + bytes.len()].copy_from_slice(bytes);
    z
}

#[test]
fn reads_stored_and_deflated_entries_and_lists_libs() {
    let apk = Apk::from_bytes(sample()).unwrap();
    assert_eq!(apk.libs(), ["libApplication.so", "libstored.so"]);
    assert_eq!(apk.names().count(), 6);
    assert_eq!(apk.read("classes.dex").unwrap(), b"dex");
    assert_eq!(apk.read_lib("libstored.so").unwrap(), b"stored-bytes");
    let big = apk.read(LIB).unwrap();
    assert_eq!(big.len(), 5000);
    assert_eq!(big[10], 3);
    assert_eq!(apk.read("nope").unwrap_err(), ApkError::Missing("nope".into()));
}

#[test]
fn open_reads_a_file_and_reports_io_errors() {
    let dir = std::env::temp_dir().join(format!("bhpc-loader-apk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.apk");
    std::fs::write(&path, sample()).unwrap();
    assert_eq!(Apk::open(&path).unwrap().libs().len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(matches!(Apk::open(&path).err().unwrap(), ApkError::Io(m) if m.contains("t.apk")));
}

#[test]
fn rejects_malformed_archives() {
    assert_eq!(Apk::from_bytes(vec![]).err().unwrap(), ApkError::NotZip);
    assert_eq!(Apk::from_bytes(vec![0; 100]).err().unwrap(), ApkError::NotZip);
    let z = sample();
    let e = eocd(&z);
    // Central directory outside the file / entry count too large for the data.
    assert_eq!(
        Apk::from_bytes(patch(z.clone(), e + 16, &0x00ff_0000u32.to_le_bytes())).err().unwrap(),
        ApkError::Truncated
    );
    assert!(matches!(
        Apk::from_bytes(patch(z.clone(), e + 10, &9u16.to_le_bytes())).err().unwrap(),
        ApkError::Corrupt(_)
    ));
    assert_eq!(
        Apk::from_bytes(patch(z.clone(), e + 10, &0xffffu16.to_le_bytes())).err().unwrap(),
        ApkError::Unsupported("zip64")
    );
    assert_eq!(
        Apk::from_bytes(patch(z.clone(), e + 16, &u32::MAX.to_le_bytes())).err().unwrap(),
        ApkError::Unsupported("zip64")
    );
    let c = cd_off(&z);
    assert!(matches!(Apk::from_bytes(patch(z.clone(), c, b"XXXX")).err().unwrap(), ApkError::Corrupt(_)));
    assert_eq!(
        Apk::from_bytes(patch(z.clone(), c + 8, &1u16.to_le_bytes())).err().unwrap(),
        ApkError::Unsupported("encryption")
    );
    assert_eq!(
        Apk::from_bytes(patch(z.clone(), c + 20, &u32::MAX.to_le_bytes())).err().unwrap(),
        ApkError::Unsupported("zip64")
    );
    assert_eq!(
        Apk::from_bytes(patch(z.clone(), c + 24, &u32::MAX.to_le_bytes())).err().unwrap(),
        ApkError::Unsupported("zip64")
    );
    // Name runs past the end of the file.
    let mut t = z.clone();
    t.truncate(e);
    let tail = t.len();
    t.extend_from_slice(&z[e..]);
    assert_eq!(Apk::from_bytes(patch(t, c + 28, &(tail as u16).to_le_bytes())).err().unwrap(), ApkError::Truncated);
    // Central directory entry fields cut off by the end of the file.
    let mut t = z[..c + 10].to_vec();
    t.extend_from_slice(&[0; 0]);
    let mut short = t.clone();
    short.extend_from_slice(&z[e..]);
    let new_e = short.len() - 22;
    let short = patch(short, new_e + 16, &(c as u32).to_le_bytes());
    let short = patch(short, new_e + 12, &0u32.to_le_bytes());
    assert_eq!(Apk::from_bytes(short).err().unwrap(), ApkError::Truncated);
}

#[test]
fn rejects_bad_entry_data() {
    let z = sample();
    let c = cd_off(&z);
    // Entry 1 is libApplication (deflated). Central directory entries have name-dependent sizes: walk them.
    let mut starts = vec![];
    let mut o = c;
    for _ in 0..6 {
        starts.push(o);
        let n = u16::from_le_bytes([z[o + 28], z[o + 29]]) as usize;
        o += 46 + n;
    }
    let app = starts[1];
    let lh = u32::from_le_bytes(z[app + 42..app + 46].try_into().unwrap()) as usize;
    let apk = |z: Vec<u8>| Apk::from_bytes(z).unwrap();
    // Unsupported method.
    assert_eq!(
        apk(patch(z.clone(), app + 10, &9u16.to_le_bytes())).read(LIB).unwrap_err(),
        ApkError::Unsupported("compression method")
    );
    // Local header signature.
    assert!(
        matches!(apk(patch(z.clone(), lh, b"XXXX")).read(LIB).unwrap_err(), ApkError::Corrupt(m) if m.contains("local header"))
    );
    // Local header offset beyond the file.
    assert!(matches!(
        apk(patch(z.clone(), app + 42, &0xffffu32.to_le_bytes())).read(LIB).unwrap_err(),
        ApkError::Corrupt(_)
    ));
    // Compressed size larger than the file.
    assert_eq!(
        apk(patch(z.clone(), app + 20, &0x00ff_ffffu32.to_le_bytes())).read(LIB).unwrap_err(),
        ApkError::Truncated
    );
    // Garbage deflate stream (stored bytes 0xff are an invalid block type).
    let data_at = lh + 30 + LIB.len();
    assert!(matches!(
        apk(patch(z.clone(), data_at, &[0xff, 0xff, 0xff, 0xff])).read(LIB).unwrap_err(),
        ApkError::Corrupt(m) if m.contains("deflate")
    ));
    // CRC and size mismatches.
    let stored = starts[2];
    assert!(
        matches!(apk(patch(z.clone(), stored + 16, &[1, 2, 3, 4])).read_lib("libstored.so").unwrap_err(), ApkError::Corrupt(m) if m.contains("CRC"))
    );
    assert!(matches!(
        apk(patch(z.clone(), stored + 24, &3u32.to_le_bytes())).read_lib("libstored.so").unwrap_err(),
        ApkError::Corrupt(_)
    ));
}

#[test]
fn builds_are_identified_by_hash() {
    for (release, hash) in KNOWN_BUILDS {
        assert_eq!(identify(hash), Some(release));
        assert_eq!(hash.len(), 64);
    }
    assert_eq!(identify("00"), None);
    assert_eq!(KNOWN_BUILDS.map(|(r, _)| r.version()), ["1.7.4", "1.7.5", "1.7.6"]);
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn verify_refuses_unknown_builds_unless_forced() {
    let apk = Apk::from_bytes(sample()).unwrap();
    let hash = sha256_hex(&apk.read(LIB).unwrap());
    assert_eq!(apk.verify(false).unwrap_err(), ApkError::UnknownBuild { sha256: hash.clone() });
    assert_eq!(apk.verify(true).unwrap(), None);
    let table = [(Release::V1_7_5, hash.as_str())];
    assert_eq!(apk.verify_with(&table, false).unwrap(), Some(Release::V1_7_5));
    let no_lib = Apk::from_bytes(build_zip(&[("classes.dex", b"x", false)])).unwrap();
    assert_eq!(no_lib.verify(true).unwrap_err(), ApkError::Missing(LIB.into()));
}

#[test]
fn apk_is_a_library_source() {
    let mut apk = Apk::from_bytes(sample()).unwrap();
    assert_eq!(apk.fetch("libstored.so").unwrap(), Some(b"stored-bytes".to_vec()));
    assert_eq!(apk.fetch("libc.so").unwrap(), None);
    let z = sample();
    let c = cd_off(&z);
    let mut o = c;
    for _ in 0..2 {
        o += 46 + u16::from_le_bytes([z[o + 28], z[o + 29]]) as usize;
    }
    let mut bad = Apk::from_bytes(patch(z, o + 16, &[9, 9, 9, 9])).unwrap();
    assert!(matches!(bad.fetch("libstored.so").unwrap_err(), LoadError::Source(m) if m.contains("CRC")));
}

#[test]
fn apk_errors_display() {
    let all = [
        ApkError::Io("x".into()),
        ApkError::NotZip,
        ApkError::Truncated,
        ApkError::Unsupported("zip64"),
        ApkError::Corrupt("c".into()),
        ApkError::Missing("m".into()),
        ApkError::UnknownBuild { sha256: "ab".into() },
    ];
    let text: Vec<String> = all.iter().map(|e| e.to_string()).collect();
    assert_eq!(text[0], "cannot read APK: x");
    assert_eq!(text[1], "not a zip/APK file");
    assert_eq!(text[2], "truncated APK");
    assert_eq!(text[3], "unsupported zip feature: zip64");
    assert_eq!(text[4], "corrupt APK: c");
    assert_eq!(text[5], "APK has no entry m");
    assert!(text[6].contains("sha256 ab") && text[6].contains("--force"));
    let _: &dyn std::error::Error = &all[0];
}

#[test]
fn every_truncation_of_the_central_directory_is_an_error_not_a_panic() {
    let z = sample();
    let c = cd_off(&z);
    for n in 0..z.len() - c {
        // Cut the file inside the central directory and re-attach a (consistent) end record.
        let mut cut = z[..c + n].to_vec();
        cut.extend_from_slice(&z[eocd(&z)..]);
        let e = cut.len() - 22;
        let cut = patch(cut, e + 12, &(n as u32).to_le_bytes());
        // Either an error or, when the cut falls on an entry boundary, a parse with fewer entries.
        if let Ok(apk) = Apk::from_bytes(cut) {
            assert!(apk.names().count() <= 6);
        }
    }
}
