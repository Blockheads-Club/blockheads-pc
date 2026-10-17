//! Tests of errors, paths, flag parsing, host and synthetic file systems.

use crate::error::VfsError;
use crate::fs::{FileKind, FileSystem, OpenFlags, Stat};
use crate::hostfs::{HostFs, SingleFile};
use crate::memfs::{MemFs, Node};
use crate::path::{components, normalize, strip_mount};
use std::io::{self, SeekFrom};

#[test]
fn errno_and_display() {
    use VfsError::*;
    let all = [
        (NotFound, 2),
        (Io, 5),
        (BadFd, 9),
        (PermissionDenied, 13),
        (Exists, 17),
        (CrossDevice, 18),
        (NotDir, 20),
        (IsDir, 21),
        (Invalid, 22),
        (ReadOnly, 30),
        (NotEmpty, 39),
    ];
    for (e, n) in all {
        assert_eq!(e.errno(), n);
        assert!(!e.to_string().is_empty());
    }
    let _: &dyn std::error::Error = &NotFound;
}

#[test]
fn io_error_mapping() {
    use io::ErrorKind as K;
    let m = |k: K| VfsError::from(io::Error::from(k));
    assert_eq!(m(K::NotFound), VfsError::NotFound);
    assert_eq!(m(K::PermissionDenied), VfsError::PermissionDenied);
    assert_eq!(m(K::AlreadyExists), VfsError::Exists);
    assert_eq!(m(K::NotADirectory), VfsError::NotDir);
    assert_eq!(m(K::IsADirectory), VfsError::IsDir);
    assert_eq!(m(K::DirectoryNotEmpty), VfsError::NotEmpty);
    assert_eq!(m(K::ReadOnlyFilesystem), VfsError::ReadOnly);
    assert_eq!(m(K::InvalidInput), VfsError::Invalid);
    assert_eq!(m(K::TimedOut), VfsError::Io);
    assert_eq!(VfsError::from(zip::result::ZipError::FileNotFound), VfsError::NotFound);
    assert_eq!(VfsError::from(zip::result::ZipError::InvalidArchive("x")), VfsError::Io);
}

#[test]
fn path_normalisation() {
    assert_eq!(normalize("/", "/a//b/./c/"), Ok("/a/b/c".into()));
    assert_eq!(normalize("/x/y", "z"), Ok("/x/y/z".into()));
    assert_eq!(normalize("/x/y", "../../../.."), Ok("/".into()));
    assert_eq!(normalize("/", "/a/../../b"), Ok("/b".into()));
    assert_eq!(normalize("/", ""), Err(VfsError::NotFound));
    assert_eq!(normalize("/", "a\0b"), Err(VfsError::Invalid));
    assert_eq!(components("/a/b").collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(components("/").count(), 0);
    assert_eq!(strip_mount("/a/b", "/"), Some("/a/b"));
    assert_eq!(strip_mount("/a/b", "/a"), Some("/b"));
    assert_eq!(strip_mount("/a", "/a"), Some("/"));
    assert_eq!(strip_mount("/ab", "/a"), None);
    assert_eq!(strip_mount("/b", "/a"), None);
}

#[test]
fn open_flag_translation() {
    let f = OpenFlags::from_bionic(0);
    assert!(f.read && !f.write && !f.create);
    let f = OpenFlags::from_bionic(1 | 0o100 | 0o1000);
    assert!(!f.read && f.write && f.create && f.truncate && !f.excl && !f.append);
    let f = OpenFlags::from_bionic(2 | 0o200 | 0o2000 | 0o40000);
    assert!(f.read && f.write && f.excl && f.append && f.directory);

    let r = OpenFlags::from_fopen_mode("rb").unwrap();
    assert!(r.read && !r.write);
    let r = OpenFlags::from_fopen_mode("r+").unwrap();
    assert!(r.read && r.write && !r.create);
    let w = OpenFlags::from_fopen_mode("w").unwrap();
    assert!(!w.read && w.write && w.create && w.truncate);
    let w = OpenFlags::from_fopen_mode("w+x").unwrap();
    assert!(w.read && w.excl);
    let a = OpenFlags::from_fopen_mode("ab").unwrap();
    assert!(!a.read && a.append && a.create && !a.truncate);
    assert!(OpenFlags::from_fopen_mode("a+").unwrap().read);
    assert_eq!(OpenFlags::from_fopen_mode("z"), Err(VfsError::Invalid));
    assert_eq!(OpenFlags::from_fopen_mode(""), Err(VfsError::Invalid));
}

#[test]
fn stat_modes() {
    let mut s = Stat::file(1);
    assert_eq!(s.mode(), 0o100444);
    s.readonly = false;
    assert_eq!(s.mode(), 0o100644);
    let mut d = Stat::dir();
    assert_eq!(d.mode(), 0o040555);
    d.readonly = false;
    assert_eq!(d.mode(), 0o040755);
}

fn rw() -> (tempfile::TempDir, HostFs) {
    let d = tempfile::tempdir().unwrap();
    let fs = HostFs::new(d.path(), true);
    (d, fs)
}

fn write_file(fs: &HostFs, path: &str, data: &[u8]) {
    let f = OpenFlags { write: true, create: true, truncate: true, ..Default::default() };
    let mut h = fs.open(path, f).unwrap();
    assert_eq!(h.write(data), Ok(data.len()));
}

fn read_all(fs: &dyn FileSystem, path: &str) -> Vec<u8> {
    let mut h = fs.open(path, OpenFlags::read_only()).unwrap();
    let mut out = Vec::new();
    let mut b = [0u8; 7];
    loop {
        let n = h.read(&mut b).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[..n]);
    }
}

#[test]
fn hostfs_read_write_roundtrip() {
    let (_d, fs) = rw();
    assert_eq!(fs.root(), _d.path());
    fs.mkdir("/saves").unwrap();
    assert_eq!(fs.mkdir("/saves"), Err(VfsError::Exists));
    write_file(&fs, "/saves/w1.dat", b"0123456789");
    assert_eq!(read_all(&fs, "/saves/w1.dat"), b"0123456789");
    let st = fs.stat("/saves/w1.dat").unwrap();
    assert_eq!((st.kind, st.size, st.readonly), (FileKind::File, 10, false));
    assert!(st.mtime > 0);
    assert_eq!(fs.stat("/saves").unwrap().kind, FileKind::Dir);
    assert_eq!(fs.stat("/saves").unwrap().size, 0);

    // append, seek, set_len, sync, len
    let mut h = fs.open("/saves/w1.dat", OpenFlags::from_fopen_mode("a").unwrap()).unwrap();
    assert_eq!(h.read(&mut [0u8; 1]), Err(VfsError::BadFd));
    h.seek(SeekFrom::Start(0)).unwrap();
    assert_eq!(h.write(b"AB"), Ok(2));
    assert_eq!(h.len(), Ok(12));
    h.sync().unwrap();
    drop(h);
    assert_eq!(read_all(&fs, "/saves/w1.dat"), b"0123456789AB");

    let mut h = fs.open("/saves/w1.dat", OpenFlags::from_fopen_mode("r+").unwrap()).unwrap();
    assert_eq!(h.seek(SeekFrom::End(-2)), Ok(10));
    h.set_len(4).unwrap();
    assert_eq!(h.len(), Ok(4));
    drop(h);

    // read-only handle refuses mutation
    let mut h = fs.open("/saves/w1.dat", OpenFlags::read_only()).unwrap();
    assert_eq!(h.write(b"x"), Err(VfsError::BadFd));
    assert_eq!(h.set_len(0), Err(VfsError::BadFd));

    // create/excl/directory/IsDir/NotFound
    let excl = OpenFlags { write: true, create: true, excl: true, ..Default::default() };
    assert_eq!(fs.open("/saves/w1.dat", excl).err(), Some(VfsError::Exists));
    let rdwr_create = OpenFlags { read: true, create: true, ..Default::default() };
    assert_eq!(fs.open("/saves/made", rdwr_create).map(|_| ()), Ok(()));
    assert_eq!(fs.open("/saves", OpenFlags::read_only()).err(), Some(VfsError::IsDir));
    assert_eq!(fs.open("/nope", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
    let dir = OpenFlags { directory: true, ..OpenFlags::read_only() };
    assert_eq!(fs.open("/saves/w1.dat", dir).err(), Some(VfsError::NotDir));
    assert_eq!(fs.open("/nope", dir).err(), Some(VfsError::NotFound));

    // directory listing
    let names: Vec<_> = fs.read_dir("/saves").unwrap().into_iter().map(|e| (e.name, e.kind)).collect();
    assert_eq!(names, [("made".to_string(), FileKind::File), ("w1.dat".to_string(), FileKind::File)]);
    assert!(fs.read_dir("/").unwrap().iter().any(|e| e.kind == FileKind::Dir));
    assert!(fs.read_dir("/nope").is_err());

    // rename / unlink / rmdir
    fs.rename("/saves/made", "/saves/moved").unwrap();
    assert_eq!(fs.stat("/saves/made"), Err(VfsError::NotFound));
    assert_eq!(fs.unlink("/saves"), Err(VfsError::IsDir));
    assert_eq!(fs.unlink("/nope"), Err(VfsError::NotFound));
    fs.unlink("/saves/moved").unwrap();
    assert_eq!(fs.rmdir("/saves"), Err(VfsError::NotEmpty));
    assert_eq!(fs.rmdir("/saves/w1.dat"), Err(VfsError::NotDir));
    fs.unlink("/saves/w1.dat").unwrap();
    fs.rmdir("/saves").unwrap();
}

#[test]
fn hostfs_refuses_dangerous_names_and_ro_mutation() {
    let (d, fs) = rw();
    for p in ["/C:/Windows", "/a\\b", "/..:/x"] {
        assert_eq!(fs.stat(p).err(), Some(VfsError::Invalid));
    }
    assert_eq!(fs.host_path("/a/b").unwrap(), d.path().join("a").join("b"));

    let ro = HostFs::new(d.path(), false);
    let w = OpenFlags { write: true, ..Default::default() };
    assert_eq!(ro.open("/x", w).err(), Some(VfsError::ReadOnly));
    assert_eq!(ro.mkdir("/x"), Err(VfsError::ReadOnly));
    assert_eq!(ro.unlink("/x"), Err(VfsError::ReadOnly));
    assert_eq!(ro.rmdir("/x"), Err(VfsError::ReadOnly));
    assert_eq!(ro.rename("/x", "/y"), Err(VfsError::ReadOnly));
    std::fs::write(d.path().join("f"), b"1").unwrap();
    let st = ro.stat("/f").unwrap();
    assert!(st.readonly);
    assert_eq!(read_all(&ro, "/f"), b"1");
}

#[test]
fn single_file_serves_only_itself() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("base.apk");
    std::fs::write(&p, b"PK-fake").unwrap();
    let s = SingleFile::new(&p);
    assert_eq!(read_all(&s, "/"), b"PK-fake");
    let st = s.stat("/").unwrap();
    assert_eq!((st.size, st.readonly, st.kind), (7, true, FileKind::File));
    assert_eq!(s.open("/x", OpenFlags::read_only()).err(), Some(VfsError::NotDir));
    assert_eq!(s.stat("/x"), Err(VfsError::NotDir));
    assert_eq!(s.read_dir("/"), Err(VfsError::NotDir));
    let w = OpenFlags { write: true, ..Default::default() };
    assert_eq!(s.open("/", w).err(), Some(VfsError::ReadOnly));
    assert_eq!(SingleFile::new(d.path().join("gone")).stat("/"), Err(VfsError::NotFound));
}

#[test]
fn memfs_nodes() {
    let mut m = MemFs::new();
    m.insert_text("/proc/cpuinfo", "abc");
    m.insert("/dev/null", Node::Null);
    m.insert("/dev/urandom", Node::Random);
    assert_eq!(read_all(&m, "/proc/cpuinfo"), b"abc");
    let mut h = m.open("/proc/cpuinfo", OpenFlags::read_only()).unwrap();
    assert_eq!(h.write(b"x"), Err(VfsError::BadFd));
    assert_eq!(h.set_len(0), Err(VfsError::BadFd));
    assert_eq!(h.sync(), Ok(()));
    assert_eq!(h.len(), Ok(3));
    assert_eq!(h.seek(SeekFrom::Start(1)), Ok(1));
    let mut b = [0u8; 8];
    assert_eq!(h.read(&mut b), Ok(2));
    assert_eq!(&b[..2], b"bc");
    assert_eq!(h.seek(SeekFrom::Current(-9)), Err(VfsError::Invalid));

    let mut n = m.open("/dev/null", OpenFlags::from_bionic(1)).unwrap();
    assert_eq!(n.write(b"zzz"), Ok(3));
    assert_eq!(n.read(&mut b), Ok(0));
    assert_eq!(n.len(), Ok(0));
    let mut r = m.open("/dev/urandom", OpenFlags::read_only()).unwrap();
    let mut big = [0u8; 64];
    assert_eq!(r.read(&mut big), Ok(64));
    assert!(big.iter().any(|&x| x != 0));
    assert_eq!(r.write(b"seed"), Ok(4));

    assert_eq!(m.stat("/proc/cpuinfo").unwrap().size, 3);
    assert!(!m.stat("/dev/null").unwrap().readonly);
    assert_eq!(m.stat("/proc").unwrap().kind, FileKind::Dir);
    assert_eq!(m.stat("/nope"), Err(VfsError::NotFound));
    let w = OpenFlags { write: true, ..Default::default() };
    assert_eq!(m.open("/proc/cpuinfo", w).err(), Some(VfsError::ReadOnly));
    let dir = OpenFlags { directory: true, ..OpenFlags::read_only() };
    assert_eq!(m.open("/proc/cpuinfo", dir).err(), Some(VfsError::NotDir));
    assert_eq!(m.open("/proc", OpenFlags::read_only()).err(), Some(VfsError::IsDir));
    assert_eq!(m.open("/nope", OpenFlags::read_only()).err(), Some(VfsError::NotFound));

    let root: Vec<_> = m.read_dir("/").unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(root, ["dev", "proc"]);
    let dev = m.read_dir("/dev").unwrap();
    assert_eq!(dev.len(), 2);
    assert!(dev.iter().all(|e| e.kind == FileKind::File));
    assert_eq!(m.read_dir("/proc/cpuinfo"), Err(VfsError::NotDir));
    assert_eq!(m.read_dir("/nope"), Err(VfsError::NotFound));
    assert_eq!(m.mkdir("/x"), Err(VfsError::ReadOnly));
}

#[test]
fn memfs_nested_dirs_list_only_direct_children() {
    let mut m = MemFs::new();
    m.insert_text("/a/b/c.txt", "1");
    m.insert_text("/a/d.txt", "2");
    m.insert_text("/ab/e.txt", "3");
    let a: Vec<_> = m.read_dir("/a").unwrap().into_iter().map(|e| (e.name, e.kind)).collect();
    assert_eq!(a, [("b".to_string(), FileKind::Dir), ("d.txt".to_string(), FileKind::File)]);
}

#[test]
fn memfs_zero_device_and_links() {
    let mut m = MemFs::new();
    m.insert("/dev/zero", Node::Zero);
    m.insert_link("/proc/self/exe", "/data/app/x/base.apk");
    m.insert_text("/proc/f", "t");
    let mut z = m.open("/dev/zero", OpenFlags::read_only()).unwrap();
    let mut b = [7u8; 8];
    assert_eq!(z.read(&mut b), Ok(8));
    assert_eq!(b, [0; 8]);
    assert_eq!(z.write(b"abc"), Ok(3));
    assert_eq!(m.readlink("/proc/self/exe").unwrap(), "/data/app/x/base.apk");
    assert_eq!(m.readlink("/proc/f"), Err(VfsError::Invalid));
    assert_eq!(m.readlink("/proc"), Err(VfsError::Invalid));
    assert_eq!(m.readlink("/nope"), Err(VfsError::NotFound));
    // A link stats as an empty file and cannot be opened here (its target is in another backend).
    assert_eq!(m.stat("/proc/self/exe").unwrap().size, 0);
    assert_eq!(m.open("/proc/self/exe", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
}
