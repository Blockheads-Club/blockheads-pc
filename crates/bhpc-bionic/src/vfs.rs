//! The guest's descriptor table on top of [`bhpc_vfs`]. All path handling, mounts, `/proc` and `/dev`
//! content and the Android-to-host mapping live in `bhpc-vfs`; this module only owns what is per
//! process: descriptor numbers, file positions (inside each [`VFile`]), directory cursors and the
//! `errno` / `struct stat` vocabulary of the libc layer.

use bhpc_vfs::{FileKind, FileSystem, OpenFlags, VFile, VfsError};
use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::sync::Arc;

pub const EPERM: u32 = 1;
pub const ENOENT: u32 = 2;
pub const EIO: u32 = 5;
pub const EBADF: u32 = 9;
pub const EACCES: u32 = 13;
pub const EEXIST: u32 = 17;
pub const ENOTDIR: u32 = 20;
pub const EISDIR: u32 = 21;
pub const EINVAL: u32 = 22;
pub const EMFILE: u32 = 24;
pub const ENOTEMPTY: u32 = 39;
pub const ESPIPE: u32 = 29;
pub const EROFS: u32 = 30;

pub const O_ACCMODE: u32 = 3;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_DIRECTORY: u32 = 0o40000;

pub const S_IFDIR: u32 = 0o040_000;
pub const S_IFCHR: u32 = 0o020_000;
pub const S_IFREG: u32 = 0o100_000;

/// Highest descriptor number handed out.
pub const MAX_FDS: i32 = 1024;

/// `stat` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub mode: u32,
    pub size: u64,
    pub mtime: i64,
    pub ino: u64,
    pub nlink: u32,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub ino: u64,
}

fn errno(e: VfsError) -> u32 {
    e.errno() as u32
}

fn stat_of(s: &bhpc_vfs::Stat) -> Stat {
    let dir = s.kind == FileKind::Dir;
    Stat {
        mode: s.mode(),
        size: if dir { 4096 } else { s.size },
        mtime: s.mtime,
        ino: if dir { 1 } else { 100 },
        nlink: 1 + u32::from(dir),
    }
}

/// The file system of a bare environment: `/proc`, `/sys`, `/dev` and `/etc` content only.
pub fn default_fs() -> Arc<dyn FileSystem> {
    let vfs = bhpc_vfs::Vfs::new();
    let sys = bhpc_vfs::SystemInfo::default();
    vfs.mount("/", Arc::new(bhpc_vfs::layout::system_files(&sys))).expect("the root is a valid mount point");
    Arc::new(vfs)
}

enum Handle {
    File(Box<dyn VFile>),
    Dir { entries: Vec<DirEntry>, pos: usize },
}

struct Open {
    handle: Handle,
    flags: u32,
    /// Path as the guest gave it, for `fstat`.
    path: String,
}

/// The descriptor table over a [`FileSystem`].
pub struct Vfs {
    fs: Arc<dyn FileSystem>,
    open: BTreeMap<i32, Open>,
    /// Log every path lookup to stderr (`--trace-fs`).
    pub trace: bool,
}

impl Vfs {
    pub fn new(fs: Arc<dyn FileSystem>) -> Vfs {
        Vfs { fs, open: BTreeMap::new(), trace: false }
    }

    /// The file system behind the descriptors.
    pub fn fs(&self) -> &Arc<dyn FileSystem> {
        &self.fs
    }

    fn alloc_fd(&self) -> Result<i32, u32> {
        (3..MAX_FDS).find(|fd| !self.open.contains_key(fd)).ok_or(EMFILE)
    }

    fn dir_handle(&self, path: &str) -> Result<Handle, u32> {
        let mut entries = vec![
            DirEntry { name: ".".into(), is_dir: true, ino: 1 },
            DirEntry { name: "..".into(), is_dir: true, ino: 1 },
        ];
        let listed = self.fs.read_dir(path).map_err(errno)?;
        entries.extend(listed.into_iter().enumerate().map(|(i, e)| DirEntry {
            name: e.name,
            is_dir: e.kind == FileKind::Dir,
            ino: 100 + i as u64,
        }));
        Ok(Handle::Dir { entries, pos: 0 })
    }

    /// `open(2)`.
    pub fn open(&mut self, path: &str, flags: u32) -> Result<i32, u32> {
        if path.is_empty() {
            return Err(ENOENT);
        }
        if self.trace {
            eprintln!("[fs] {path}");
        }
        let wants_write = flags & O_ACCMODE != 0 || flags & (O_CREAT | O_TRUNC) != 0;
        let handle = if flags & O_DIRECTORY != 0 {
            if wants_write {
                return Err(EISDIR);
            }
            match self.fs.stat(path).map_err(errno)?.kind {
                FileKind::File => return Err(ENOTDIR),
                FileKind::Dir => self.dir_handle(path)?,
            }
        } else {
            match self.fs.open(path, OpenFlags::from_bionic(flags)) {
                Ok(f) => Handle::File(f),
                Err(VfsError::IsDir) if !wants_write => self.dir_handle(path)?,
                Err(e) => return Err(errno(e)),
            }
        };
        let fd = self.alloc_fd()?;
        self.open.insert(fd, Open { handle, flags, path: path.to_string() });
        Ok(fd)
    }

    /// True if `fd` is an open descriptor.
    pub fn is_open(&self, fd: i32) -> bool {
        self.open.contains_key(&fd)
    }

    pub fn close(&mut self, fd: i32) -> Result<(), u32> {
        self.open.remove(&fd).map(|_| ()).ok_or(EBADF)
    }

    /// Read up to `n` bytes at the current position (fewer only at the end of the file).
    pub fn read(&mut self, fd: i32, n: usize) -> Result<Vec<u8>, u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        if o.flags & O_ACCMODE == O_WRONLY {
            return Err(EBADF);
        }
        let Handle::File(f) = &mut o.handle else { return Err(EISDIR) };
        let mut buf = vec![0; n];
        let mut got = 0;
        while got < n {
            match f.read(&mut buf[got..]).map_err(errno)? {
                0 => break,
                k => got += k,
            }
        }
        buf.truncate(got);
        Ok(buf)
    }

    pub fn write(&mut self, fd: i32, data: &[u8]) -> Result<usize, u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        if o.flags & O_ACCMODE == 0 {
            return Err(EBADF);
        }
        let Handle::File(f) = &mut o.handle else { return Err(EISDIR) };
        let mut done = 0;
        while done < data.len() {
            match f.write(&data[done..]).map_err(errno)? {
                0 => return Err(EIO),
                k => done += k,
            }
        }
        Ok(data.len())
    }

    /// `lseek(2)`; whence 0/1/2 = SET/CUR/END. Returns the new position.
    pub fn lseek(&mut self, fd: i32, off: i64, whence: u32) -> Result<u64, u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        match &mut o.handle {
            Handle::File(f) => {
                let from = match whence {
                    0 => SeekFrom::Start(u64::try_from(off).map_err(|_| EINVAL)?),
                    1 => SeekFrom::Current(off),
                    2 => SeekFrom::End(off),
                    _ => return Err(EINVAL),
                };
                f.seek(from).map_err(errno)
            }
            Handle::Dir { pos, entries } => {
                // rewinddir / telldir style positioning.
                let base = if whence == 1 { *pos as i64 } else { 0 };
                let np = u64::try_from(base.checked_add(off).ok_or(EINVAL)?).map_err(|_| EINVAL)? as usize;
                *pos = np.min(entries.len());
                Ok(*pos as u64)
            }
        }
    }

    pub fn ftruncate(&mut self, fd: i32, len: u64) -> Result<(), u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        match &mut o.handle {
            Handle::File(f) => f.set_len(len).map_err(errno),
            Handle::Dir { .. } => Err(EISDIR),
        }
    }

    pub fn fsync(&mut self, fd: i32) -> Result<(), u32> {
        match &mut self.open.get_mut(&fd).ok_or(EBADF)?.handle {
            Handle::File(f) => f.sync().map_err(errno),
            Handle::Dir { .. } => Ok(()),
        }
    }

    pub fn stat(&self, path: &str) -> Result<Stat, u32> {
        if path.is_empty() {
            return Err(ENOENT);
        }
        self.fs.stat(path).map(|s| stat_of(&s)).map_err(errno)
    }

    pub fn fstat(&mut self, fd: i32) -> Result<Stat, u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        if let Ok(s) = self.fs.stat(&o.path) {
            return Ok(stat_of(&s));
        }
        // The name is gone (unlinked while open): describe the open file itself.
        match &mut o.handle {
            Handle::File(f) => {
                let size = f.len().map_err(errno)?;
                Ok(Stat { mode: S_IFREG | 0o644, size, mtime: 0, ino: 100, nlink: 0 })
            }
            Handle::Dir { .. } => Ok(Stat { mode: S_IFDIR | 0o755, size: 4096, mtime: 0, ino: 1, nlink: 2 }),
        }
    }

    /// `access(2)`: existence, and write permission only where something is writable.
    pub fn access(&self, path: &str, mode: u32) -> Result<(), u32> {
        if path.is_empty() {
            return Err(ENOENT);
        }
        let s = self.fs.stat(path).map_err(errno)?;
        if mode & 2 != 0 && s.readonly {
            return Err(EACCES);
        }
        Ok(())
    }

    pub fn unlink(&mut self, path: &str) -> Result<(), u32> {
        self.fs.unlink(path).map_err(errno)
    }

    pub fn rmdir(&mut self, path: &str) -> Result<(), u32> {
        self.fs.rmdir(path).map_err(errno)
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), u32> {
        self.fs.mkdir(path).map_err(errno)
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), u32> {
        self.fs.rename(from, to).map_err(errno)
    }

    /// Next directory entry of an open directory.
    pub fn readdir(&mut self, fd: i32) -> Result<Option<DirEntry>, u32> {
        match &mut self.open.get_mut(&fd).ok_or(EBADF)?.handle {
            Handle::Dir { entries, pos } => {
                let e = entries.get(*pos).cloned();
                *pos += usize::from(e.is_some());
                Ok(e)
            }
            Handle::File(_) => Err(ENOTDIR),
        }
    }

    /// The bytes `[off, off + len)` of the file behind `fd`, zero padded; the position is unchanged.
    pub fn read_range(&mut self, fd: i32, off: u64, len: usize) -> Result<Vec<u8>, u32> {
        let saved = self.lseek(fd, 0, 1)?;
        self.lseek(fd, off as i64, 0)?;
        let r = self.read(fd, len);
        self.lseek(fd, saved as i64, 0)?;
        let mut data = r?;
        data.resize(len, 0);
        Ok(data)
    }

    /// `readlink`: only `/proc/self/exe` is a link.
    pub fn readlink(&self, path: &str) -> Result<String, u32> {
        self.fs.readlink(path).map_err(errno)
    }
}

/// A file system with the synthetic files of package `pkg` and a writable `/data/data/pkg` backed by `host`.
#[cfg(test)]
pub(crate) fn test_fs(host: std::path::PathBuf) -> Arc<dyn FileSystem> {
    let vfs = bhpc_vfs::Vfs::new();
    let sys =
        bhpc_vfs::SystemInfo { package: "pkg".into(), exe: "/data/app/pkg-1/base.apk".into(), ..Default::default() };
    vfs.mount("/", Arc::new(bhpc_vfs::layout::system_files(&sys))).unwrap();
    vfs.mount("/data/data/pkg", Arc::new(bhpc_vfs::hostfs::HostFs::new(host, true))).unwrap();
    Arc::new(vfs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhpc_vfs::hostfs::HostFs;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A fresh scratch directory removed on drop.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "bhpc-fd-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// `/data/data/pkg` (writable) and `/ro` (read-only) mounted from `tmp`, plus the synthetic files.
    fn vfs_with(tmp: &Tmp) -> Vfs {
        fs::create_dir_all(tmp.path("rw")).unwrap();
        fs::create_dir_all(tmp.path("ro")).unwrap();
        let v = bhpc_vfs::Vfs::new();
        let sys = bhpc_vfs::SystemInfo {
            package: "pkg".into(),
            exe: "/data/app/pkg-1/base.apk".into(),
            pid: 4242,
            ..Default::default()
        };
        v.mount("/", Arc::new(bhpc_vfs::layout::system_files(&sys))).unwrap();
        v.mount("/data/data/pkg", Arc::new(HostFs::new(tmp.path("rw"), true))).unwrap();
        v.mount("/ro", Arc::new(HostFs::new(tmp.path("ro"), false))).unwrap();
        Vfs::new(Arc::new(v))
    }

    /// A file that accepts nothing and whose name is gone.
    struct Stuck;
    impl VFile for Stuck {
        fn read(&mut self, _: &mut [u8]) -> bhpc_vfs::Result<usize> {
            Ok(0)
        }
        fn write(&mut self, _: &[u8]) -> bhpc_vfs::Result<usize> {
            Ok(0)
        }
        fn seek(&mut self, _: SeekFrom) -> bhpc_vfs::Result<u64> {
            Ok(0)
        }
        fn len(&mut self) -> bhpc_vfs::Result<u64> {
            Ok(7)
        }
        fn set_len(&mut self, _: u64) -> bhpc_vfs::Result<()> {
            Ok(())
        }
        fn sync(&mut self) -> bhpc_vfs::Result<()> {
            Ok(())
        }
    }

    /// A file system whose single file opens fine but cannot be `stat`ed afterwards.
    struct Ghost;
    impl FileSystem for Ghost {
        fn open(&self, path: &str, _: OpenFlags) -> bhpc_vfs::Result<Box<dyn VFile>> {
            if path == "/dir" {
                return Err(VfsError::IsDir);
            }
            Ok(Box::new(Stuck))
        }
        fn stat(&self, _: &str) -> bhpc_vfs::Result<bhpc_vfs::Stat> {
            Err(VfsError::NotFound)
        }
        fn read_dir(&self, _: &str) -> bhpc_vfs::Result<Vec<bhpc_vfs::DirEntry>> {
            Err(VfsError::NotFound)
        }
    }

    #[test]
    fn virtual_files_and_directories() {
        let tmp = Tmp::new();
        let mut v = vfs_with(&tmp);
        let fd = v.open("/proc/self/cmdline", 0).unwrap();
        assert_eq!(v.read(fd, 100).unwrap(), b"pkg\0");
        assert_eq!(v.read(fd, 10).unwrap(), b"", "at end of file");
        assert_eq!(v.lseek(fd, 1, 0).unwrap(), 1);
        assert_eq!(v.lseek(fd, 2, 1).unwrap(), 3);
        assert_eq!(v.lseek(fd, -1, 2).unwrap(), 3);
        assert_eq!(v.lseek(fd, -100, 0), Err(EINVAL));
        assert_eq!(v.lseek(fd, 0, 9), Err(EINVAL));
        assert_eq!(v.fstat(fd).unwrap().mode, S_IFREG | 0o444);
        assert_eq!(v.write(fd, b"x"), Err(EBADF), "opened read-only");
        v.close(fd).unwrap();
        assert_eq!(v.close(fd), Err(EBADF));
        // By pid as well as "self".
        assert!(v.open("/proc/4242/cmdline", 0).is_ok());
        assert_eq!(v.open("/proc/1/cmdline", 0), Err(ENOENT));
        for (path, needle) in
            [("/proc/cpuinfo", "neon"), ("/proc/meminfo", "MemTotal"), ("/sys/devices/system/cpu/present", "0-3")]
        {
            let fd = v.open(path, 0).unwrap();
            let text = String::from_utf8(v.read(fd, 4096).unwrap()).unwrap();
            assert!(text.contains(needle), "{path}");
        }
        // Writing or creating virtual files fails; directories cannot be opened for writing.
        assert_eq!(v.open("/proc/cpuinfo", O_WRONLY), Err(EROFS));
        assert_eq!(v.open("/proc/cpuinfo", O_DIRECTORY), Err(ENOTDIR));
        assert_eq!(v.open("/proc", O_WRONLY), Err(EISDIR));
        assert_eq!(v.open("/proc", O_DIRECTORY | O_RDWR), Err(EISDIR));
        assert_eq!(v.open("", 0), Err(ENOENT));
        assert_eq!(v.stat(""), Err(ENOENT));
        assert_eq!(v.access("", 0), Err(ENOENT));
        assert_eq!(v.open("/nowhere", 0), Err(ENOENT));
        assert_eq!(v.open("/nowhere", O_DIRECTORY), Err(ENOENT));
        // Listing.
        for dir in ["/", "/proc", "/dev", "/sys", "/system", "/data", "/data/data"] {
            let fd = v.open(dir, O_DIRECTORY).unwrap();
            let first = v.readdir(fd).unwrap().unwrap();
            assert_eq!(first.name, ".", "{dir}");
            assert_eq!(v.readdir(fd).unwrap().unwrap().name, "..");
            while v.readdir(fd).unwrap().is_some() {}
            assert_eq!(v.readdir(fd).unwrap(), None, "stays at the end");
            assert_eq!(v.lseek(fd, 0, 0).unwrap(), 0, "rewinddir");
            assert_eq!(v.readdir(fd).unwrap().unwrap().name, ".");
            assert!(v.lseek(fd, 1000, 1).unwrap() >= 1, "clamped to the end");
            assert_eq!(v.lseek(fd, -1000, 1), Err(EINVAL));
            assert_eq!(v.lseek(fd, i64::MAX, 1), Err(EINVAL));
            assert_eq!(v.read(fd, 1), Err(EISDIR));
            assert_eq!(v.write(fd, b"x"), Err(EBADF), "directories are read-only handles");
            assert_eq!(v.ftruncate(fd, 0), Err(EISDIR));
            v.fsync(fd).unwrap();
            assert_eq!(v.fstat(fd).unwrap().mode, S_IFDIR | 0o555);
        }
        // Opening a directory without O_DIRECTORY works too; a directory handle opened for writing does not.
        let d = v.open("/data/data", 0).unwrap();
        assert_eq!(v.readdir(d).unwrap().unwrap().name, ".");
        let f = v.open("/proc/cpuinfo", 0).unwrap();
        assert_eq!(v.readdir(f), Err(ENOTDIR));
        assert_eq!(v.readdir(999), Err(EBADF));
        // The tracing switch only prints.
        v.trace = true;
        assert!(v.open("/proc/cpuinfo", 0).is_ok());
    }

    #[test]
    fn devices() {
        let tmp = Tmp::new();
        let mut v = vfs_with(&tmp);
        let null = v.open("/dev/null", O_RDWR).unwrap();
        assert_eq!(v.write(null, b"abc").unwrap(), 3);
        assert_eq!(v.read(null, 8).unwrap(), b"");
        let zero = v.open("/dev/zero", 0).unwrap();
        assert_eq!(v.read(zero, 4).unwrap(), [0; 4]);
        let rnd = v.open("/dev/urandom", 0).unwrap();
        let a = v.read(rnd, 16).unwrap();
        let b = v.read(rnd, 16).unwrap();
        assert_eq!((a.len(), b.len()), (16, 16));
        assert_ne!(a, b);
        let rnd2 = v.open("/dev/random", O_WRONLY).unwrap();
        assert_eq!(v.write(rnd2, b"seed").unwrap(), 4);
        assert_eq!(v.read(rnd2, 1), Err(EBADF), "write-only descriptor");
        assert!(v.access("/dev/null", 2).is_ok());
        assert_eq!(v.access("/dev/null", 1), Ok(()));
        assert!(v.ftruncate(null, 0).is_err());
        v.fsync(null).unwrap();
        assert_eq!(v.fsync(77), Err(EBADF));
        assert_eq!(v.ftruncate(77, 0), Err(EBADF));
        assert_eq!(v.fstat(77), Err(EBADF));
        assert_eq!(v.lseek(77, 0, 0), Err(EBADF));
        assert_eq!(v.read(77, 1), Err(EBADF));
        assert_eq!(v.write(77, b"x"), Err(EBADF));
    }

    #[test]
    fn host_files_read_write_seek_truncate() {
        let tmp = Tmp::new();
        let mut v = vfs_with(&tmp);
        assert_eq!(v.open("/data/data/pkg/f.txt", 0), Err(ENOENT), "missing without O_CREAT");
        let fd = v.open("/data/data/pkg/f.txt", O_RDWR | O_CREAT).unwrap();
        assert_eq!(v.write(fd, b"hello world").unwrap(), 11);
        assert_eq!(v.lseek(fd, 0, 0).unwrap(), 0);
        assert_eq!(v.read(fd, 5).unwrap(), b"hello");
        assert_eq!(v.read(fd, 100).unwrap(), b" world");
        assert_eq!(v.lseek(fd, -5, 2).unwrap(), 6);
        assert_eq!(v.lseek(fd, -1, 0), Err(EINVAL));
        assert_eq!(v.lseek(fd, 0, 7), Err(EINVAL));
        assert_eq!(v.read_range(fd, 3, 4).unwrap(), b"lo w");
        assert_eq!(v.read_range(fd, 9, 6).unwrap(), b"ld\0\0\0\0", "zero padded past the end");
        assert_eq!(v.lseek(fd, 0, 1).unwrap(), 6, "read_range restores the offset");
        v.ftruncate(fd, 5).unwrap();
        v.fsync(fd).unwrap();
        assert_eq!(v.fstat(fd).unwrap().size, 5);
        assert_eq!(v.stat("/data/data/pkg/f.txt").unwrap().size, 5);
        v.close(fd).unwrap();
        assert_eq!(fs::read(tmp.path("rw/f.txt")).unwrap(), b"hello");
        // Truncate on open, append mode, write-only vs read-only.
        let fd = v.open("/data/data/pkg/f.txt", O_WRONLY | O_TRUNC).unwrap();
        assert_eq!(v.read(fd, 1), Err(EBADF));
        v.write(fd, b"ab").unwrap();
        v.close(fd).unwrap();
        let fd = v.open("/data/data/pkg/f.txt", O_WRONLY | O_APPEND).unwrap();
        v.write(fd, b"cd").unwrap();
        v.close(fd).unwrap();
        assert_eq!(fs::read(tmp.path("rw/f.txt")).unwrap(), b"abcd");
        let fd = v.open("/data/data/pkg/f.txt", 0).unwrap();
        assert_eq!(v.write(fd, b"x"), Err(EBADF));
        assert_eq!(v.ftruncate(fd, 0), Err(EBADF), "read-only descriptor");
        // Exclusive creation and directories.
        assert_eq!(v.open("/data/data/pkg/f.txt", O_CREAT | O_EXCL | O_WRONLY), Err(EEXIST));
        assert_eq!(v.open("/data/data/pkg/f.txt", O_DIRECTORY), Err(ENOTDIR));
        assert_eq!(v.open("/data/data/pkg", O_WRONLY), Err(EISDIR));
        assert_eq!(v.open("/data/data/pkg/nodir/x", O_CREAT | O_WRONLY), Err(ENOENT));
    }

    #[test]
    fn host_directories_and_file_management() {
        let tmp = Tmp::new();
        let mut v = vfs_with(&tmp);
        v.mkdir("/data/data/pkg/sub").unwrap();
        assert_eq!(v.mkdir("/data/data/pkg/sub"), Err(EEXIST));
        assert_eq!(v.mkdir("/data/data/pkg/a/b"), Err(ENOENT));
        assert_eq!(v.mkdir("/data"), Err(EEXIST), "virtual directories exist already");
        assert_eq!(v.mkdir("/ro/x"), Err(EROFS));
        let fd = v.open("/data/data/pkg/sub/a", O_WRONLY | O_CREAT).unwrap();
        v.close(fd).unwrap();
        fs::write(tmp.path("rw/sub/b"), b"x").unwrap();
        // Directory listing is sorted and typed.
        let d = v.open("/data/data/pkg/sub", O_DIRECTORY).unwrap();
        let mut names = Vec::new();
        while let Some(e) = v.readdir(d).unwrap() {
            names.push((e.name, e.is_dir));
        }
        assert_eq!(names, [(".".into(), true), ("..".into(), true), ("a".into(), false), ("b".into(), false)]);
        assert_eq!(v.rmdir("/data/data/pkg/sub"), Err(ENOTEMPTY));
        assert_eq!(v.unlink("/data/data/pkg/sub"), Err(EISDIR));
        v.rename("/data/data/pkg/sub/a", "/data/data/pkg/sub/c").unwrap();
        assert_eq!(v.rename("/data/data/pkg/sub/a", "/data/data/pkg/sub/d"), Err(ENOENT));
        assert_eq!(v.rename("/data/data/pkg/sub/c", "/ro/c"), Err(18), "EXDEV across mounts");
        v.unlink("/data/data/pkg/sub/c").unwrap();
        v.unlink("/data/data/pkg/sub/b").unwrap();
        assert_eq!(v.unlink("/data/data/pkg/sub/b"), Err(ENOENT));
        v.rmdir("/data/data/pkg/sub").unwrap();
        assert_eq!(v.unlink("/proc/cpuinfo"), Err(EROFS));
        assert_eq!(v.rmdir("/ro"), Err(EROFS));
        assert_eq!(v.rmdir("/nothing/here"), Err(ENOENT));
        // Access and stat.
        assert!(v.access("/data/data/pkg", 2).is_ok());
        assert!(v.access("/data/data/pkg", 4).is_ok());
        assert_eq!(v.access("/ro", 2), Err(EACCES));
        assert!(v.access("/ro", 4).is_ok());
        assert_eq!(v.access("/data/data/pkg/none", 0), Err(ENOENT));
        assert!(v.access("/proc/cpuinfo", 4).is_ok());
        assert_eq!(v.access("/proc/cpuinfo", 2), Err(EACCES));
        assert_eq!(v.stat("/data/data/pkg").unwrap().mode, S_IFDIR | 0o755);
        assert_eq!(v.stat("/data/data/pkg").unwrap().nlink, 2);
        assert_eq!(v.stat("/proc/cpuinfo").unwrap().mode, S_IFREG | 0o444);
        assert_eq!(v.stat("/none"), Err(ENOENT));
        // Read-only mount.
        fs::write(tmp.path("ro/r.txt"), b"readonly").unwrap();
        assert_eq!(v.open("/ro/r.txt", O_WRONLY), Err(EROFS));
        assert_eq!(v.open("/ro/r.txt", O_CREAT), Err(EROFS));
        let fd = v.open("/ro/r.txt", 0).unwrap();
        assert_eq!(v.read(fd, 100).unwrap(), b"readonly");
        assert_eq!(v.stat("/ro/r.txt").unwrap().mode, S_IFREG | 0o444);
        // Mounts match whole path components, and intermediate directories exist.
        assert_eq!(v.open("/ro2/x", 0), Err(ENOENT));
        assert_eq!(v.stat("/data").unwrap().mode, S_IFDIR | 0o555);
        assert_eq!(v.stat("/data/datax"), Err(ENOENT));
    }

    #[test]
    fn readlink_and_descriptor_limits() {
        let tmp = Tmp::new();
        let mut v = vfs_with(&tmp);
        assert_eq!(v.readlink("/proc/self/exe").unwrap(), "/data/app/pkg-1/base.apk");
        assert!(v.readlink("/proc/4242/exe").is_ok());
        assert_eq!(v.readlink("/proc/cpuinfo"), Err(EINVAL));
        assert_eq!(v.readlink("/nothing"), Err(ENOENT));
        // Descriptors start at 3 and the table is finite.
        let first = v.open("/dev/null", 0).unwrap();
        assert_eq!(first, 3);
        let mut last = first;
        while let Ok(fd) = v.open("/dev/null", 0) {
            last = fd;
        }
        assert_eq!(last, MAX_FDS - 1);
        assert_eq!(v.open("/dev/null", 0), Err(EMFILE));
        v.close(5).unwrap();
        assert_eq!(v.open("/dev/null", 0).unwrap(), 5, "the lowest free descriptor is reused");
        assert!(v.is_open(5) && !v.is_open(MAX_FDS));
        assert!(v.fs().stat("/proc/cpuinfo").is_ok());
    }

    #[test]
    fn default_environment_has_the_synthetic_files() {
        let mut v = Vfs::new(default_fs());
        let fd = v.open("/proc/self/cmdline", 0).unwrap();
        assert_eq!(v.read(fd, 100).unwrap(), b"com.noodlecake.blockheads\0");
        assert_eq!(v.stat("/data/data/pkg"), Err(ENOENT));
    }

    #[test]
    fn files_that_make_no_progress_or_lost_their_name() {
        let mut v = Vfs::new(Arc::new(Ghost));
        let fd = v.open("/x", O_RDWR).unwrap();
        assert_eq!(v.write(fd, b"abc"), Err(EIO), "a file that accepts nothing is an I/O error");
        assert_eq!(v.write(fd, b""), Ok(0));
        assert_eq!(v.read(fd, 4).unwrap(), b"");
        // fstat falls back to the open file when the name no longer resolves.
        let st = v.fstat(fd).unwrap();
        assert_eq!((st.mode, st.size, st.nlink), (S_IFREG | 0o644, 7, 0));
        assert_eq!(v.lseek(fd, 3, 0), Ok(0));
        assert_eq!(v.ftruncate(fd, 0), Ok(()));
        assert_eq!(v.fsync(fd), Ok(()));
        assert_eq!(v.open("/d", O_DIRECTORY), Err(ENOENT));
        assert_eq!(v.open("/dir", 0), Err(ENOENT), "a directory that cannot be listed");
        // A directory handle whose directory vanished still reports as a directory.
        v.open.insert(20, Open { handle: Handle::Dir { entries: vec![], pos: 0 }, flags: 0, path: "/gone".into() });
        assert_eq!(v.fstat(20).unwrap().mode, S_IFDIR | 0o755);
    }
}
