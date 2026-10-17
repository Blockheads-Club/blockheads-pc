//! Tests of the mount table.

use crate::error::VfsError;
use crate::fs::{FileKind, FileSystem, OpenFlags};
use crate::hostfs::{HostFs, SingleFile};
use crate::memfs::MemFs;
use crate::vfs::Vfs;
use std::sync::Arc;

fn setup() -> (tempfile::TempDir, Vfs) {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("a")).unwrap();
    std::fs::create_dir_all(d.path().join("b")).unwrap();
    std::fs::write(d.path().join("a/f.txt"), b"in a").unwrap();
    std::fs::write(d.path().join("apk"), b"APK").unwrap();
    let v = Vfs::default();
    v.mount("/data/data/pkg/files", Arc::new(HostFs::new(d.path().join("a"), true))).unwrap();
    v.mount("/data/data/pkg", Arc::new(HostFs::new(d.path().join("b"), true))).unwrap();
    v.mount("/data/app/pkg-1/base.apk", Arc::new(SingleFile::new(d.path().join("apk")))).unwrap();
    let mut m = MemFs::new();
    m.insert_text("/proc/cpuinfo", "cpu");
    v.mount("/", Arc::new(m)).unwrap();
    (d, v)
}

fn names(v: &Vfs, p: &str) -> Vec<(String, FileKind)> {
    v.read_dir(p).unwrap().into_iter().map(|e| (e.name, e.kind)).collect()
}

#[test]
fn longest_prefix_wins_and_paths_are_normalised() {
    let (_d, v) = setup();
    let mut f = v.open("/data/data/pkg/files/../files/./f.txt", OpenFlags::read_only()).unwrap();
    let mut b = [0u8; 8];
    assert_eq!(f.read(&mut b), Ok(4));
    assert_eq!(v.stat("/data/data/pkg/files/f.txt").unwrap().size, 4);
    // Outer mount serves everything not under the inner one.
    v.mkdir("/data/data/pkg/databases").unwrap();
    assert_eq!(names(&v, "/data/data/pkg").iter().map(|n| n.0.as_str()).collect::<Vec<_>>(), ["databases", "files"]);
    // `..` cannot climb out of the root.
    assert_eq!(v.stat("/../../proc/cpuinfo").unwrap().size, 3);
    assert_eq!(v.mount_points().len(), 4);
    assert_eq!(v.resolve(""), Err(VfsError::NotFound));
}

#[test]
fn virtual_parent_directories() {
    let (_d, v) = setup();
    assert_eq!(v.stat("/data").unwrap().kind, FileKind::Dir);
    assert_eq!(v.stat("/data/data").unwrap().kind, FileKind::Dir);
    assert_eq!(names(&v, "/data"), [("app".into(), FileKind::Dir), ("data".into(), FileKind::Dir)]);
    // Mount point that is a file is listed as a file.
    assert_eq!(names(&v, "/data/app/pkg-1"), [("base.apk".into(), FileKind::File)]);
    assert_eq!(v.stat("/data/app/pkg-1/base.apk").unwrap().size, 3);
    // Root listing merges the backend ("proc") with mount children ("data").
    assert_eq!(names(&v, "/"), [("data".into(), FileKind::Dir), ("proc".into(), FileKind::Dir)]);
    assert_eq!(v.open("/data", OpenFlags::read_only()).err(), Some(VfsError::IsDir));
    assert_eq!(v.open("/data/app", OpenFlags::read_only()).err(), Some(VfsError::IsDir));
    assert_eq!(v.stat("/nope"), Err(VfsError::NotFound));
    assert_eq!(v.read_dir("/nope").err(), Some(VfsError::NotFound));
    assert_eq!(v.open("/nope", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
    // The backend has no "/data": the lookup falls back to the virtual directory.
    assert_eq!(v.mkdir("/data"), Err(VfsError::Exists));
    // A read-only backend: existing paths are EEXIST, new ones EROFS, and removing nothing is ENOENT.
    assert_eq!(v.mkdir("/proc"), Err(VfsError::Exists));
    assert_eq!(v.mkdir("/proc/new"), Err(VfsError::ReadOnly));
    assert_eq!(v.unlink("/proc/cpuinfo"), Err(VfsError::ReadOnly));
    assert_eq!(v.unlink("/proc/none"), Err(VfsError::NotFound));
    assert_eq!(v.rmdir("/proc/none"), Err(VfsError::NotFound));
    // readlink without any backend below the path.
    let empty = Vfs::default();
    assert_eq!(empty.readlink("/x"), Err(VfsError::NotFound));
}

#[test]
fn mutation_through_mounts() {
    let (d, v) = setup();
    let w = OpenFlags { write: true, create: true, ..Default::default() };
    v.open("/data/data/pkg/files/new.txt", w).unwrap().write(b"hi").unwrap();
    assert_eq!(std::fs::read(d.path().join("a/new.txt")).unwrap(), b"hi");
    v.rename("/data/data/pkg/files/new.txt", "/data/data/pkg/files/old.txt").unwrap();
    v.unlink("/data/data/pkg/files/old.txt").unwrap();
    v.mkdir("/data/data/pkg/files/sub").unwrap();
    v.rmdir("/data/data/pkg/files/sub").unwrap();
    // Cross-mount rename.
    assert_eq!(v.rename("/data/data/pkg/files/f.txt", "/data/data/pkg/f.txt"), Err(VfsError::CrossDevice));
    // Read-only synthetic root.
    assert_eq!(v.unlink("/proc/cpuinfo"), Err(VfsError::ReadOnly));
}

#[test]
fn unmounted_mutations() {
    let d = tempfile::tempdir().unwrap();
    let v = Vfs::new();
    v.mount("/data/x", Arc::new(HostFs::new(d.path(), true))).unwrap();
    assert_eq!(v.mkdir("/data"), Err(VfsError::Exists));
    assert_eq!(v.mkdir("/other"), Err(VfsError::NotFound));
    assert_eq!(v.unlink("/data"), Err(VfsError::PermissionDenied));
    assert_eq!(v.rmdir("/data"), Err(VfsError::PermissionDenied));
    assert_eq!(v.unlink("/other"), Err(VfsError::NotFound));
    assert_eq!(v.rmdir("/other"), Err(VfsError::NotFound));
    assert_eq!(v.rename("/data", "/data/x/y"), Err(VfsError::PermissionDenied));
    assert_eq!(v.rename("/data/x/a", "/data"), Err(VfsError::PermissionDenied));
    assert_eq!(v.rename("/data/x/a", "/zzz"), Err(VfsError::NotFound));
    assert_eq!(v.stat("/data").unwrap().kind, FileKind::Dir);
    assert_eq!(v.stat("/other"), Err(VfsError::NotFound));
    assert_eq!(v.open("/data", OpenFlags::read_only()).err(), Some(VfsError::IsDir));
    assert_eq!(v.open("/other", OpenFlags::read_only()).err(), Some(VfsError::NotFound));
    assert_eq!(v.read_dir("/other").err(), Some(VfsError::NotFound));
    assert_eq!(v.read_dir("/data").unwrap().len(), 1);
    assert_eq!(v.read_dir("/").unwrap().len(), 1);
}

#[test]
fn cwd_and_relative_paths() {
    let (_d, v) = setup();
    assert_eq!(v.cwd(), "/");
    v.chdir("/data/data/pkg/files").unwrap();
    assert_eq!(v.cwd(), "/data/data/pkg/files");
    assert_eq!(v.stat("f.txt").unwrap().size, 4);
    assert_eq!(v.chdir("f.txt"), Err(VfsError::NotDir));
    assert_eq!(v.chdir("missing"), Err(VfsError::NotFound));
    v.chdir("..").unwrap();
    assert_eq!(v.cwd(), "/data/data/pkg");
}

#[test]
fn misses_are_recorded_and_bounded() {
    let (_d, v) = setup();
    let _ = v.stat("/system/lib/libfoo.so");
    let _ = v.open("/system/lib/libfoo.so", OpenFlags::read_only());
    let _ = v.read_dir("/nope");
    let _ = v.stat("/proc/cpuinfo");
    let m = v.take_misses();
    assert_eq!(m, ["open /system/lib/libfoo.so", "readdir /nope", "stat /system/lib/libfoo.so"]);
    assert!(v.take_misses().is_empty());
    for i in 0..5000 {
        let _ = v.stat(&format!("/nope/{i}"));
    }
    assert_eq!(v.take_misses().len(), 4096);
}

#[test]
fn mount_replaces_same_prefix() {
    let v = Vfs::new();
    let mut a = MemFs::new();
    a.insert_text("/x", "1");
    let mut b = MemFs::new();
    b.insert_text("/y", "2");
    v.mount("/m", Arc::new(a)).unwrap();
    v.mount("/m/", Arc::new(b)).unwrap();
    assert_eq!(v.mount_points(), ["/m"]);
    assert!(v.stat("/m/y").is_ok() && v.stat("/m/x").is_err());
    assert_eq!(v.mount("", Arc::new(MemFs::new())), Err(VfsError::NotFound));
}
