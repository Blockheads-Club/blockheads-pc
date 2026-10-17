//! Host-directory backends: the writable user data tree and single read-only host files.

use crate::error::{Result, VfsError};
use crate::fs::{DirEntry, FileKind, FileSystem, OpenFlags, Stat, VFile};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// An open host file.
pub struct HostFile {
    file: File,
    append: bool,
    readable: bool,
    writable: bool,
}

impl VFile for HostFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.readable {
            return Err(VfsError::BadFd);
        }
        Ok(self.file.read(buf)?)
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        if !self.writable {
            return Err(VfsError::BadFd);
        }
        if self.append {
            self.file.seek(SeekFrom::End(0))?;
        }
        Ok(self.file.write(buf)?)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        Ok(self.file.seek(pos)?)
    }

    fn len(&mut self) -> Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    fn set_len(&mut self, len: u64) -> Result<()> {
        if !self.writable {
            return Err(VfsError::BadFd);
        }
        Ok(self.file.set_len(len)?)
    }

    fn sync(&mut self) -> Result<()> {
        Ok(self.file.sync_all()?)
    }
}

fn stat_of(meta: &fs::Metadata, force_readonly: bool) -> Stat {
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    Stat {
        kind: if meta.is_dir() { FileKind::Dir } else { FileKind::File },
        size: if meta.is_dir() { 0 } else { meta.len() },
        mtime,
        readonly: force_readonly || meta.permissions().readonly(),
    }
}

fn open_host(path: &Path, flags: OpenFlags, writable_fs: bool) -> Result<Box<dyn VFile>> {
    let wants_write = flags.write || flags.create || flags.truncate || flags.append;
    if wants_write && !writable_fs {
        return Err(VfsError::ReadOnly);
    }
    let meta = fs::metadata(path);
    if matches!(&meta, Ok(m) if m.is_dir()) {
        return Err(VfsError::IsDir);
    }
    if flags.directory {
        return Err(if meta.is_ok() { VfsError::NotDir } else { VfsError::NotFound });
    }
    let mut o = OpenOptions::new();
    o.read(flags.read || !flags.write).write(flags.write).append(false).truncate(flags.truncate && flags.write);
    if flags.create {
        if flags.excl {
            o.create_new(true);
        } else {
            o.create(true);
        }
        // `create` needs write access on the host even for `O_RDONLY|O_CREAT`.
        o.write(true);
    }
    let file = o.open(path)?;
    Ok(Box::new(HostFile { file, append: flags.append, readable: flags.read || !flags.write, writable: flags.write }))
}

/// A host directory exposed as a file system, optionally writable.
///
/// Guest paths are already normalised (no `..`), so they cannot leave the root. Components
/// containing `\` or `:` are refused so that a guest name can never become a Windows drive or
/// UNC path.
pub struct HostFs {
    root: PathBuf,
    writable: bool,
}

impl HostFs {
    /// Exposes `root` (which must exist for lookups to succeed).
    pub fn new(root: impl Into<PathBuf>, writable: bool) -> Self {
        HostFs { root: root.into(), writable }
    }

    /// The host root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Host path for a backend-relative guest path.
    pub fn host_path(&self, path: &str) -> Result<PathBuf> {
        let mut p = self.root.clone();
        for c in crate::path::components(path) {
            if c.contains(['\\', ':']) {
                return Err(VfsError::Invalid);
            }
            p.push(c);
        }
        Ok(p)
    }

    fn mutate(&self, path: &str) -> Result<PathBuf> {
        if !self.writable {
            return Err(VfsError::ReadOnly);
        }
        self.host_path(path)
    }
}

impl FileSystem for HostFs {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>> {
        open_host(&self.host_path(path)?, flags, self.writable)
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        Ok(stat_of(&fs::metadata(self.host_path(path)?)?, !self.writable))
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.host_path(path)?)? {
            let e = e?;
            let kind = if e.metadata()?.is_dir() { FileKind::Dir } else { FileKind::File };
            out.push(DirEntry { name: e.file_name().to_string_lossy().into_owned(), kind });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn mkdir(&self, path: &str) -> Result<()> {
        Ok(fs::create_dir(self.mutate(path)?)?)
    }

    fn unlink(&self, path: &str) -> Result<()> {
        let p = self.mutate(path)?;
        if fs::metadata(&p)?.is_dir() {
            return Err(VfsError::IsDir);
        }
        Ok(fs::remove_file(p)?)
    }

    fn rmdir(&self, path: &str) -> Result<()> {
        let p = self.mutate(path)?;
        if !fs::metadata(&p)?.is_dir() {
            return Err(VfsError::NotDir);
        }
        Ok(fs::remove_dir(p)?)
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (a, b) = (self.mutate(from)?, self.mutate(to)?);
        Ok(fs::rename(a, b)?)
    }
}

/// One read-only host file, mounted where the guest expects it (the APK at
/// `/data/app/<pkg>-1/base.apk`, so libv's zzip can parse the zip through plain `open`/`read`/`lseek`).
pub struct SingleFile {
    path: PathBuf,
}

impl SingleFile {
    /// Serves `path` at the mount point itself.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        SingleFile { path: path.into() }
    }
}

impl FileSystem for SingleFile {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>> {
        if path != "/" {
            return Err(VfsError::NotDir);
        }
        open_host(&self.path, flags, false)
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        if path != "/" {
            return Err(VfsError::NotDir);
        }
        Ok(stat_of(&fs::metadata(&self.path)?, true))
    }

    fn read_dir(&self, _path: &str) -> Result<Vec<DirEntry>> {
        Err(VfsError::NotDir)
    }
}
