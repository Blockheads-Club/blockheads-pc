//! Shared helpers and tests of archive, asset and zip file system.

use crate::archive::ApkArchive;
use crate::asset::{AssetManager, Whence};
use crate::error::VfsError;
use crate::fs::{FileKind, FileSystem, OpenFlags};
use crate::zipfs::ZipFs;
use std::io::{Cursor, Write};
use std::sync::Arc;

/// A tiny fake APK: stored and deflated entries, an explicit directory entry and implicit parents.
pub fn fake_apk() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    w.start_file("assets/hello.txt", stored).unwrap();
    w.write_all(b"hello world").unwrap();
    w.start_file("assets/GameResources/Fonts/a.fnt", deflated).unwrap();
    w.write_all(&b"deflate me ".repeat(100)).unwrap();
    w.start_file("assets/GameResources/Fonts/b.fnt", stored).unwrap();
    w.write_all(b"b").unwrap();
    w.add_directory("assets/empty/", stored).unwrap();
    w.start_file("com.apportable.system/fonts/DroidSansThai.ttf", stored).unwrap();
    w.write_all(b"THAI").unwrap();
    w.start_file("com.apportable.system/fonts/DroidSansHebrew-Regular.ttf", stored).unwrap();
    w.write_all(b"HEBREW").unwrap();
    w.start_file("com.apportable.system/fonts/DroidNaskh-Regular-SystemUI.ttf", stored).unwrap();
    w.write_all(b"NASKH").unwrap();
    w.start_file("com.apportable.system/icu/icu.dat", stored).unwrap();
    w.write_all(b"ICU").unwrap();
    w.start_file("/", stored).unwrap();
    w.start_file("lib/armeabi-v7a/libv.so", stored).unwrap();
    w.write_all(b"\x7fELF").unwrap();
    w.finish().unwrap().into_inner()
}

pub fn apk() -> Arc<ApkArchive> {
    Arc::new(ApkArchive::from_bytes(fake_apk()).unwrap())
}

#[test]
fn archive_index_and_read() {
    let a = apk();
    assert_eq!(a.file_count(), 8);
    assert!(a.file_names().contains(&"assets/hello.txt".to_string()));
    assert_eq!(a.file_size("assets/hello.txt"), Some(11));
    assert_eq!(a.file_size("/assets/hello.txt/"), Some(11));
    assert_eq!(a.file_size("nope"), None);
    assert!(a.is_dir("") && a.is_dir("assets") && a.is_dir("assets/GameResources/Fonts/"));
    assert!(a.is_dir("assets/empty"));
    assert!(!a.is_dir("assets/hello.txt"));
    assert!(a.is_file("assets/hello.txt") && !a.is_file("assets"));
    assert_eq!(
        a.list("assets").unwrap(),
        vec![("GameResources".to_string(), true), ("empty".to_string(), true), ("hello.txt".to_string(), false)]
    );
    assert!(a.list("nope").is_none());
    assert_eq!(&*a.read("assets/hello.txt").unwrap(), b"hello world");
    assert_eq!(a.read("assets/GameResources/Fonts/a.fnt").unwrap().len(), 1100);
    assert_eq!(a.read("nope"), Err(VfsError::NotFound));
}

#[test]
fn archive_shares_live_buffers_and_drops_dead_ones() {
    let a = apk();
    let x = a.read("assets/hello.txt").unwrap();
    let y = a.read("assets/hello.txt").unwrap();
    assert!(Arc::ptr_eq(&x, &y));
    drop((x, y));
    let z = a.read("assets/hello.txt").unwrap();
    assert_eq!(&*z, b"hello world");
    // A different read prunes the dead entry.
    drop(z);
    a.read("assets/GameResources/Fonts/b.fnt").unwrap();
}

#[test]
fn archive_open_from_path_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.apk");
    std::fs::write(&p, fake_apk()).unwrap();
    let a = ApkArchive::open(&p).unwrap();
    assert_eq!(a.file_count(), 8);
    assert_eq!(ApkArchive::open(&dir.path().join("missing")).err(), Some(VfsError::NotFound));
    assert_eq!(ApkArchive::from_bytes(b"not a zip".to_vec()).err(), Some(VfsError::Io));
}

#[test]
fn archive_detects_corrupt_entry() {
    let mut bytes = fake_apk();
    // Flip a byte inside the stored "hello world" payload: the CRC check must fail.
    let at = bytes.windows(11).position(|w| w == b"hello world").unwrap();
    bytes[at] ^= 0xff;
    let a = ApkArchive::from_bytes(bytes).unwrap();
    assert_eq!(a.read("assets/hello.txt"), Err(VfsError::Io));
}

#[test]
fn asset_read_seek_buffer() {
    let m = AssetManager::android(apk());
    assert!(m.exists("hello.txt") && !m.exists("GameResources") && !m.exists("zzz"));
    assert_eq!(m.length("hello.txt"), Some(11));
    let mut a = m.open("/hello.txt").unwrap();
    assert_eq!((a.length(), a.remaining()), (11, 11));
    let mut buf = [0u8; 5];
    assert_eq!(a.read(&mut buf), 5);
    assert_eq!(&buf, b"hello");
    assert_eq!(a.remaining(), 6);
    assert_eq!(a.buffer(), b"hello world");
    assert_eq!(a.seek(-5, Whence::End), Ok(6));
    let mut big = [0u8; 100];
    assert_eq!(a.read(&mut big), 5);
    assert_eq!(&big[..5], b"world");
    assert_eq!(a.read(&mut big), 0);
    assert_eq!(a.seek(0, Whence::Set), Ok(0));
    assert_eq!(a.seek(3, Whence::Cur), Ok(3));
    assert_eq!(a.seek(11, Whence::Set), Ok(11));
    assert_eq!(a.seek(12, Whence::Set), Err(VfsError::Invalid));
    assert_eq!(a.seek(-1, Whence::Set), Err(VfsError::Invalid));
    assert_eq!(a.seek(-12, Whence::Cur), Err(VfsError::Invalid));
    assert_eq!(a.remaining(), 0);
    assert_eq!(m.open("missing").err(), Some(VfsError::NotFound));
    assert_eq!(m.archive().file_count(), 8);
}

#[test]
fn whence_from_raw() {
    assert_eq!(Whence::from_raw(0), Ok(Whence::Set));
    assert_eq!(Whence::from_raw(1), Ok(Whence::Cur));
    assert_eq!(Whence::from_raw(2), Ok(Whence::End));
    assert_eq!(Whence::from_raw(3), Err(VfsError::Invalid));
}

#[test]
fn asset_dirs() {
    let m = AssetManager::android(apk());
    let mut d = m.open_dir("GameResources/Fonts").unwrap();
    assert_eq!(d.names(), ["a.fnt", "b.fnt"]);
    assert_eq!(d.next_file_name(), Some("a.fnt"));
    assert_eq!(d.next_file_name(), Some("b.fnt"));
    assert_eq!(d.next_file_name(), None);
    d.rewind();
    assert_eq!(d.next_file_name(), Some("a.fnt"));
    // Root listing skips directories for AAssetDir but not for Java list().
    assert_eq!(m.open_dir("").unwrap().names(), ["hello.txt"]);
    assert_eq!(m.list("").unwrap().len(), 3);
    assert_eq!(m.open_dir("nope").err(), Some(VfsError::NotFound));
    // A manager without a root addresses the whole zip.
    let whole = AssetManager::new(apk(), "");
    assert!(whole.exists("assets/hello.txt"));
    assert_eq!(whole.list("").unwrap().len(), 3);
}

#[test]
fn asset_as_vfile() {
    use crate::fs::VFile;
    use std::io::SeekFrom;
    let m = AssetManager::android(apk());
    let mut f = m.open("hello.txt").unwrap();
    let mut buf = [0u8; 4];
    assert_eq!(VFile::read(&mut f, &mut buf), Ok(4));
    assert_eq!(f.write(b"x"), Err(VfsError::BadFd));
    assert_eq!(f.set_len(0), Err(VfsError::BadFd));
    assert_eq!(f.sync(), Ok(()));
    assert_eq!(f.len(), Ok(11));
    assert_eq!(f.is_empty(), Ok(false));
    assert_eq!(VFile::seek(&mut f, SeekFrom::Start(100)), Ok(100));
    assert_eq!(VFile::read(&mut f, &mut buf), Ok(0));
    assert_eq!(f.remaining(), 0);
    assert_eq!(VFile::seek(&mut f, SeekFrom::Current(-95)), Ok(5));
    assert_eq!(VFile::seek(&mut f, SeekFrom::End(-1)), Ok(10));
    assert_eq!(VFile::seek(&mut f, SeekFrom::Current(-11)), Err(VfsError::Invalid));
}

#[test]
fn zipfs_behaves_like_a_read_only_fs() {
    let z = ZipFs::new(apk(), "assets");
    let st = z.stat("/hello.txt").unwrap();
    assert_eq!((st.kind, st.size, st.readonly, st.mode()), (FileKind::File, 11, true, 0o100444));
    assert_eq!(z.stat("/").unwrap().mode(), 0o040555);
    assert_eq!(z.stat("/GameResources").unwrap().kind, FileKind::Dir);
    assert_eq!(z.stat("/nope"), Err(VfsError::NotFound));
    let mut f = z.open("/hello.txt", OpenFlags::read_only()).unwrap();
    let mut b = [0u8; 11];
    assert_eq!(f.read(&mut b), Ok(11));
    assert!(z.open("/GameResources", OpenFlags::read_only()).is_err());
    assert_eq!(z.open("/nope", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
    let dir = OpenFlags { directory: true, ..OpenFlags::read_only() };
    assert_eq!(z.open("/hello.txt", dir).err(), Some(VfsError::NotDir));
    assert_eq!(z.open("/nope", dir).err(), Some(VfsError::NotFound));
    for flags in [
        OpenFlags { write: true, ..Default::default() },
        OpenFlags { create: true, ..OpenFlags::read_only() },
        OpenFlags { truncate: true, ..OpenFlags::read_only() },
        OpenFlags { append: true, ..OpenFlags::read_only() },
    ] {
        assert_eq!(z.open("/hello.txt", flags).err(), Some(VfsError::ReadOnly));
    }
    let names: Vec<_> = z.read_dir("/GameResources/Fonts").unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["a.fnt", "b.fnt"]);
    let root = z.read_dir("/").unwrap();
    assert!(root.iter().any(|e| e.name == "empty" && e.kind == FileKind::Dir));
    assert!(root.iter().any(|e| e.name == "hello.txt" && e.kind == FileKind::File));
    assert_eq!(z.read_dir("/hello.txt"), Err(VfsError::NotDir));
    assert_eq!(z.read_dir("/nope"), Err(VfsError::NotFound));
    assert_eq!(z.mkdir("/x"), Err(VfsError::ReadOnly));
    assert_eq!(z.unlink("/hello.txt"), Err(VfsError::ReadOnly));
    assert_eq!(z.rmdir("/empty"), Err(VfsError::ReadOnly));
    assert_eq!(z.rename("/a", "/b"), Err(VfsError::ReadOnly));
    // A zip-wide view has no prefix.
    let all = ZipFs::new(apk(), "");
    assert_eq!(all.stat("/assets/hello.txt").unwrap().size, 11);
}
