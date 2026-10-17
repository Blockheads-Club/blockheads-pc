//! The traits every backend implements, plus the bionic flag translations the libc layer needs.

use crate::error::{Result, VfsError};
use std::io::SeekFrom;

/// Kind of a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Regular file (also devices and synthetic files).
    File,
    /// Directory.
    Dir,
}

/// What `stat` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    /// File or directory.
    pub kind: FileKind,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Modification time, seconds since the epoch (0 when unknown).
    pub mtime: i64,
    /// True when the guest cannot write it.
    pub readonly: bool,
}

impl Stat {
    /// A read-only regular file of `size` bytes.
    pub fn file(size: u64) -> Self {
        Stat { kind: FileKind::File, size, mtime: 0, readonly: true }
    }

    /// A read-only directory.
    pub fn dir() -> Self {
        Stat { kind: FileKind::Dir, size: 0, mtime: 0, readonly: true }
    }

    /// The `st_mode` value (type bits plus permissions).
    pub fn mode(&self) -> u32 {
        match (self.kind, self.readonly) {
            (FileKind::File, true) => 0o100444,
            (FileKind::File, false) => 0o100644,
            (FileKind::Dir, true) => 0o040555,
            (FileKind::Dir, false) => 0o040755,
        }
    }
}

/// One `readdir` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Entry name (no slashes).
    pub name: String,
    /// File or directory.
    pub kind: FileKind,
}

/// Parsed `open`/`fopen` request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenFlags {
    /// Reading allowed.
    pub read: bool,
    /// Writing allowed.
    pub write: bool,
    /// Create when missing.
    pub create: bool,
    /// Fail when it exists (with `create`).
    pub excl: bool,
    /// Truncate on open.
    pub truncate: bool,
    /// Every write goes to the end.
    pub append: bool,
    /// `O_DIRECTORY`.
    pub directory: bool,
}

impl OpenFlags {
    /// Plain read-only open.
    pub fn read_only() -> Self {
        OpenFlags { read: true, ..Default::default() }
    }

    /// Translates bionic ARM `open(2)` flags (`O_ACCMODE` 3, `O_CREAT` 0100, `O_EXCL` 0200,
    /// `O_TRUNC` 01000, `O_APPEND` 02000, `O_DIRECTORY` 040000; everything else is ignored).
    pub fn from_bionic(flags: u32) -> Self {
        let acc = flags & 3;
        OpenFlags {
            read: acc == 0 || acc == 2,
            write: acc == 1 || acc == 2,
            create: flags & 0o100 != 0,
            excl: flags & 0o200 != 0,
            truncate: flags & 0o1000 != 0,
            append: flags & 0o2000 != 0,
            directory: flags & 0o40000 != 0,
        }
    }

    /// Translates a C `fopen` mode (`r`, `w`, `a`, optional `+`, optional `b`, `x`).
    pub fn from_fopen_mode(mode: &str) -> Result<Self> {
        let plus = mode.contains('+');
        let mut f = match mode.chars().next() {
            Some('r') => OpenFlags { read: true, write: plus, ..Default::default() },
            Some('w') => OpenFlags { read: plus, write: true, create: true, truncate: true, ..Default::default() },
            Some('a') => OpenFlags { read: plus, write: true, create: true, append: true, ..Default::default() },
            _ => return Err(VfsError::Invalid),
        };
        f.excl = mode.contains('x');
        Ok(f)
    }
}

/// An open file. Positions are per object; the libc layer keeps one per guest fd.
pub trait VFile: Send {
    /// Reads up to `buf.len()` bytes; 0 means end of file.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    /// Writes `buf`, returning the number of bytes accepted.
    fn write(&mut self, buf: &[u8]) -> Result<usize>;
    /// Moves the position; seeking before 0 is `EINVAL`.
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;
    /// Current size (`fstat`).
    fn len(&mut self) -> Result<u64>;
    /// `ftruncate`.
    fn set_len(&mut self, len: u64) -> Result<()>;
    /// `fsync`.
    fn sync(&mut self) -> Result<()>;

    /// True when the file is empty.
    fn is_empty(&mut self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// A file system. Paths are normalised absolute guest-style paths relative to the mount root
/// (`/` is the root of the backend). Everything but `open`, `stat` and `read_dir` defaults to
/// `EROFS`, so read-only backends stay small.
pub trait FileSystem: Send + Sync {
    /// Opens (and with the flags creates or truncates) a file.
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>>;
    /// `stat`.
    fn stat(&self, path: &str) -> Result<Stat>;
    /// Lists a directory, sorted by name, without `.` and `..`.
    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>>;
    /// `mkdir` (one level).
    fn mkdir(&self, _path: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
    /// `unlink`.
    fn unlink(&self, _path: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
    /// `rmdir`.
    fn rmdir(&self, _path: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
    /// `rename` within this backend.
    fn rename(&self, _from: &str, _to: &str) -> Result<()> {
        Err(VfsError::ReadOnly)
    }
}

/// A fully in-memory, read-only cursor shared by assets, synthetic files and zip entries.
pub(crate) fn seek_in(len: u64, pos: u64, to: SeekFrom) -> Result<u64> {
    let base = match to {
        SeekFrom::Start(n) => return Ok(n),
        SeekFrom::Current(d) => (pos as i128, d),
        SeekFrom::End(d) => (len as i128, d),
    };
    let new = base.0 + base.1 as i128;
    if new < 0 {
        Err(VfsError::Invalid)
    } else {
        Ok(new as u64)
    }
}
