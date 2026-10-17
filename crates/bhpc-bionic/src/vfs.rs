//! The guest's file system: a small set of virtual files (`/proc`, `/dev`, `/sys`) plus host directories
//! mounted into the guest namespace. Everything else does not exist. All paths are normalised lexically
//! before use, so a guest cannot escape a mount with `..`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

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

/// A host directory visible in the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Absolute, normalised guest path.
    pub guest: String,
    pub host: PathBuf,
    pub writable: bool,
}

/// File-system configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsConfig {
    /// Android package name (the process "command line" and the app directory name).
    pub package: String,
    pub mounts: Vec<Mount>,
}

impl Default for FsConfig {
    fn default() -> Self {
        FsConfig { package: "com.noodlecake.blockheads".to_string(), mounts: Vec::new() }
    }
}

impl FsConfig {
    /// Mount `root/data` as `/data/data/<package>` and `root/sdcard` as `/sdcard`, both writable.
    pub fn with_data_root(mut self, root: &std::path::Path) -> FsConfig {
        let app = format!("/data/data/{}", self.package);
        self.mounts.push(Mount { guest: app, host: root.join("data"), writable: true });
        self.mounts.push(Mount { guest: "/sdcard".to_string(), host: root.join("sdcard"), writable: true });
        self
    }
}

impl FsConfig {
    /// Guest path of the installed APK (`PACKAGE_CODE_PATH`).
    pub fn apk_path(&self) -> String {
        format!("/data/app/{}-1.apk", self.package)
    }

    /// Mount the host file `apk` read-only at [`FsConfig::apk_path`].
    pub fn with_apk(mut self, apk: &std::path::Path) -> FsConfig {
        let guest = self.apk_path();
        self.mounts.push(Mount { guest, host: apk.to_path_buf(), writable: false });
        self
    }

    /// Host sub-directories that [`FsConfig::with_data_root`] expects to exist.
    pub fn data_dirs(&self) -> Vec<String> {
        vec![
            "data/files".to_string(),
            "data/cache".to_string(),
            "sdcard/Download".to_string(),
            format!("sdcard/Android/data/{}/cache", self.package),
            format!("sdcard/Android/obb/{}", self.package),
        ]
    }

    /// Environment variables Apportable's Java-side loader provides to the native libraries.
    pub fn env(&self) -> Vec<(String, String)> {
        let p = &self.package;
        let e = |k: &str, v: String| (k.to_string(), v);
        vec![
            e("TEMPDIR", format!("/data/data/{p}/cache")),
            e("CACHEDIR", format!("/data/data/{p}/cache")),
            e("EXTERNAL_CACHEDIR", format!("/sdcard/Android/data/{p}/cache")),
            e("OBBDIR", format!("/sdcard/Android/obb/{p}")),
            e("PACKAGE_CODE_PATH", self.apk_path()),
            e("PACKAGE_RESOURCE_PATH", self.apk_path()),
            e("PACKAGE_NAME", p.clone()),
            e("CFProcessPath", format!("{}/{}", self.apk_path(), p.rsplit('.').next().unwrap_or("app"))),
            e("ANDROID_DATA_DIRECTORY", format!("/data/data/{p}/files")),
            e("ANDROID_DOWNLOAD_DIRECTORY", "/sdcard/Download".to_string()),
            e("ANDROID_EXTERNAL_STORAGE", "/sdcard".to_string()),
            e("HOME", format!("/data/data/{p}")),
            e("TMPDIR", format!("/data/data/{p}/cache")),
            e("DEBUG_BUILD", "0".to_string()),
            e("NSUnrecognizedForwardingDisabled", "1".to_string()),
            e("VERSION_CODE", "1".to_string()),
            e("LOCALE", "en_US".to_string()),
            e("LANGUAGES", "en".to_string()),
            e("TZ", "UTC".to_string()),
        ]
    }
}

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

enum Handle {
    /// Read-only in-memory file.
    Mem {
        data: Vec<u8>,
        pos: usize,
    },
    /// `/dev/null`.
    Null,
    /// `/dev/zero`.
    Zero,
    /// `/dev/urandom`, `/dev/random`.
    Random,
    Host {
        file: fs::File,
    },
    Dir {
        entries: Vec<DirEntry>,
        pos: usize,
    },
}

struct Open {
    handle: Handle,
    flags: u32,
}

enum Target {
    Virtual(Vec<u8>),
    VirtualDir(Vec<DirEntry>),
    Device(Handle),
    Host { path: PathBuf, writable: bool },
}

/// The file system plus the descriptor table.
pub struct Vfs {
    cfg: FsConfig,
    pid: u32,
    open: BTreeMap<i32, Open>,
    cwd: String,
    rng: u64,
    /// Log every path lookup to stderr (`--trace-fs`).
    pub trace: bool,
}

/// Map a host I/O error onto a Linux `errno`.
pub fn errno_of(e: &std::io::Error) -> u32 {
    use std::io::ErrorKind::*;
    match e.raw_os_error() {
        Some(n) if n > 0 => n as u32,
        _ => match e.kind() {
            NotFound => ENOENT,
            PermissionDenied => EACCES,
            AlreadyExists => EEXIST,
            InvalidInput => EINVAL,
            _ => EIO,
        },
    }
}

/// Lexically normalise `path` against `cwd`: absolute, no `.`, `..` or empty components.
pub fn normalize(cwd: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let full = if path.starts_with('/') { path.to_string() } else { format!("{cwd}/{path}") };
    for p in full.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

const CPUINFO: &str = "Processor\t: ARMv7 Processor rev 0 (v7l)\nprocessor\t: 0\nBogoMIPS\t: 38.40\n\nFeatures\t: swp half thumb fastmult vfp edsp neon vfpv3 tls vfpv4 idiva idivt\nCPU implementer\t: 0x41\nCPU architecture: 7\nCPU variant\t: 0x0\nCPU part\t: 0xc07\nCPU revision\t: 0\n\nHardware\t: bhpc\nRevision\t: 0000\nSerial\t\t: 0000000000000000\n";
const MEMINFO: &str = "MemTotal:        2048000 kB\nMemFree:         1024000 kB\nBuffers:           32768 kB\nCached:           512000 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n";

impl Vfs {
    pub fn new(cfg: FsConfig, pid: u32) -> Vfs {
        Vfs {
            cfg,
            pid,
            open: BTreeMap::new(),
            cwd: "/".to_string(),
            rng: 0x9E37_79B9_7F4A_7C15 ^ u64::from(pid),
            trace: false,
        }
    }

    pub fn config(&self) -> &FsConfig {
        &self.cfg
    }

    fn virtual_file(&self, p: &str) -> Option<Vec<u8>> {
        let cmdline = format!("{}\0", self.cfg.package).into_bytes();
        let pid = self.pid.to_string();
        let proc_pid = |rest: &str| -> Option<&str> {
            let r = p.strip_prefix("/proc/")?;
            let r = r.strip_prefix("self").or_else(|| r.strip_prefix(pid.as_str()))?;
            r.strip_prefix('/').filter(|x| *x == rest)
        };
        if proc_pid("cmdline").is_some() {
            return Some(cmdline);
        }
        match p {
            "/proc/cpuinfo" => Some(CPUINFO.as_bytes().to_vec()),
            "/proc/meminfo" => Some(MEMINFO.as_bytes().to_vec()),
            "/sys/devices/system/cpu/present" | "/sys/devices/system/cpu/possible" => Some(b"0-3\n".to_vec()),
            _ => None,
        }
    }

    fn virtual_dir(&self, p: &str) -> Option<Vec<DirEntry>> {
        let d = |names: &[&str]| -> Vec<DirEntry> {
            names.iter().map(|n| DirEntry { name: (*n).to_string(), is_dir: true, ino: 1 }).collect()
        };
        let f = |names: &[&str]| -> Vec<DirEntry> {
            names.iter().map(|n| DirEntry { name: (*n).to_string(), is_dir: false, ino: 2 }).collect()
        };
        let mut e = match p {
            "/" => d(&["data", "dev", "proc", "sdcard", "sys", "system"]),
            "/proc" => [d(&["self"]), f(&["cpuinfo", "meminfo"])].concat(),
            "/dev" => f(&["null", "zero", "urandom", "random"]),
            "/sys" => d(&["devices"]),
            "/system" => Vec::new(),
            "/data" => d(&["data"]),
            "/data/data" => d(&[self.cfg.package.as_str()]),
            _ => return None,
        };
        e.insert(0, DirEntry { name: "..".into(), is_dir: true, ino: 1 });
        e.insert(0, DirEntry { name: ".".into(), is_dir: true, ino: 1 });
        Some(e)
    }

    /// Mounts match whole path components; the longest guest prefix wins.
    fn mount_for(&self, p: &str) -> Option<(&Mount, PathBuf)> {
        self.cfg
            .mounts
            .iter()
            .filter(|m| p == m.guest || p.strip_prefix(m.guest.as_str()).is_some_and(|r| r.starts_with('/')))
            .max_by_key(|m| m.guest.len())
            .map(|m| {
                let rest = p[m.guest.len()..].trim_start_matches('/');
                (m, if rest.is_empty() { m.host.clone() } else { m.host.join(rest) })
            })
    }

    fn resolve(&self, path: &str) -> Result<(String, Target), u32> {
        if path.is_empty() {
            return Err(ENOENT);
        }
        let p = normalize(&self.cwd, path);
        if self.trace {
            eprintln!("[fs] {p}");
        }
        let t = match p.as_str() {
            "/dev/null" => Target::Device(Handle::Null),
            "/dev/zero" => Target::Device(Handle::Zero),
            "/dev/urandom" | "/dev/random" => Target::Device(Handle::Random),
            _ => {
                if let Some((m, host)) = self.mount_for(&p) {
                    Target::Host { path: host, writable: m.writable }
                } else if let Some(d) = self.virtual_file(&p) {
                    Target::Virtual(d)
                } else if let Some(e) = self.virtual_dir(&p) {
                    Target::VirtualDir(e)
                } else if self.is_ancestor_of_mount(&p) {
                    Target::VirtualDir(Vec::new())
                } else {
                    return Err(ENOENT);
                }
            }
        };
        Ok((p, t))
    }

    fn is_ancestor_of_mount(&self, p: &str) -> bool {
        self.cfg.mounts.iter().any(|m| m.guest.starts_with(&format!("{}/", p.trim_end_matches('/'))))
    }

    fn alloc_fd(&self) -> Result<i32, u32> {
        (3..MAX_FDS).find(|fd| !self.open.contains_key(fd)).ok_or(EMFILE)
    }

    fn next_random(&mut self) -> u8 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 24) as u8
    }

    /// `open(2)`.
    pub fn open(&mut self, path: &str, flags: u32) -> Result<i32, u32> {
        let (_, target) = self.resolve(path)?;
        let wants_write = flags & O_ACCMODE != 0 || flags & (O_CREAT | O_TRUNC) != 0;
        let handle = match target {
            Target::Device(h) => h,
            Target::Virtual(data) => {
                if flags & O_DIRECTORY != 0 {
                    return Err(ENOTDIR);
                }
                if wants_write {
                    return Err(if flags & O_CREAT != 0 && flags & O_EXCL != 0 { EEXIST } else { EACCES });
                }
                Handle::Mem { data, pos: 0 }
            }
            Target::VirtualDir(entries) => {
                if wants_write {
                    return Err(EISDIR);
                }
                Handle::Dir { entries, pos: 0 }
            }
            Target::Host { path: hp, writable } => {
                if wants_write && !writable {
                    return Err(EROFS);
                }
                let meta = fs::metadata(&hp);
                if let Ok(m) = &meta {
                    if m.is_dir() {
                        if wants_write {
                            return Err(EISDIR);
                        }
                        let entries = self.host_dir(&hp)?;
                        return self.install(Handle::Dir { entries, pos: 0 }, flags);
                    }
                    if flags & O_DIRECTORY != 0 {
                        return Err(ENOTDIR);
                    }
                    if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                        return Err(EEXIST);
                    }
                } else if flags & O_CREAT == 0 {
                    return Err(ENOENT);
                }
                let mut oo = fs::OpenOptions::new();
                oo.read(flags & O_ACCMODE != O_WRONLY)
                    .write(wants_write)
                    .create(flags & O_CREAT != 0)
                    .truncate(flags & O_TRUNC != 0)
                    .append(flags & O_APPEND != 0 && flags & O_ACCMODE != 0);
                let file = oo.open(&hp).map_err(|e| errno_of(&e))?;
                Handle::Host { file }
            }
        };
        self.install(handle, flags)
    }

    fn install(&mut self, handle: Handle, flags: u32) -> Result<i32, u32> {
        let fd = self.alloc_fd()?;
        self.open.insert(fd, Open { handle, flags });
        Ok(fd)
    }

    fn host_dir(&self, hp: &std::path::Path) -> Result<Vec<DirEntry>, u32> {
        let mut entries = vec![
            DirEntry { name: ".".into(), is_dir: true, ino: 1 },
            DirEntry { name: "..".into(), is_dir: true, ino: 1 },
        ];
        let mut names: Vec<_> = fs::read_dir(hp)
            .map_err(|e| errno_of(&e))?
            .filter_map(Result::ok)
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().is_ok_and(|t| t.is_dir())))
            .collect();
        names.sort();
        entries.extend(names.into_iter().enumerate().map(|(i, (name, is_dir))| DirEntry {
            name,
            is_dir,
            ino: 100 + i as u64,
        }));
        Ok(entries)
    }

    /// True if `fd` is an open descriptor.
    pub fn is_open(&self, fd: i32) -> bool {
        self.open.contains_key(&fd)
    }

    pub fn close(&mut self, fd: i32) -> Result<(), u32> {
        self.open.remove(&fd).map(|_| ()).ok_or(EBADF)
    }

    /// Read up to `n` bytes at the current position.
    pub fn read(&mut self, fd: i32, n: usize) -> Result<Vec<u8>, u32> {
        let rng: Vec<u8> = if matches!(self.open.get(&fd).map(|o| &o.handle), Some(Handle::Random)) {
            (0..n).map(|_| self.next_random()).collect()
        } else {
            Vec::new()
        };
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        if o.flags & O_ACCMODE == O_WRONLY {
            return Err(EBADF);
        }
        match &mut o.handle {
            Handle::Mem { data, pos } => {
                let end = (*pos + n).min(data.len());
                let out = data[*pos..end].to_vec();
                *pos = end;
                Ok(out)
            }
            Handle::Null => Ok(Vec::new()),
            Handle::Zero => Ok(vec![0; n]),
            Handle::Random => Ok(rng),
            Handle::Host { file } => {
                let mut buf = vec![0; n];
                let mut got = 0;
                while got < n {
                    match file.read(&mut buf[got..]) {
                        Ok(0) => break,
                        Ok(k) => got += k,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(errno_of(&e)),
                    }
                }
                buf.truncate(got);
                Ok(buf)
            }
            Handle::Dir { .. } => Err(EISDIR),
        }
    }

    pub fn write(&mut self, fd: i32, data: &[u8]) -> Result<usize, u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        if o.flags & O_ACCMODE == 0 {
            return Err(EBADF);
        }
        match &mut o.handle {
            Handle::Null | Handle::Zero | Handle::Random => Ok(data.len()),
            Handle::Host { file } => file.write_all(data).map(|()| data.len()).map_err(|e| errno_of(&e)),
            Handle::Mem { .. } => Err(EBADF),
            Handle::Dir { .. } => Err(EISDIR),
        }
    }

    /// `lseek(2)`; whence 0/1/2 = SET/CUR/END. Returns the new position.
    pub fn lseek(&mut self, fd: i32, off: i64, whence: u32) -> Result<u64, u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        let seek = |base: i64, off: i64| -> Result<u64, u32> {
            u64::try_from(base.checked_add(off).ok_or(EINVAL)?).map_err(|_| EINVAL)
        };
        match &mut o.handle {
            Handle::Mem { data, pos } => {
                let base = match whence {
                    0 => 0,
                    1 => *pos as i64,
                    2 => data.len() as i64,
                    _ => return Err(EINVAL),
                };
                *pos = seek(base, off)? as usize;
                Ok(*pos as u64)
            }
            Handle::Null | Handle::Zero | Handle::Random => Ok(0),
            Handle::Host { file } => {
                let from = match whence {
                    0 => SeekFrom::Start(u64::try_from(off).map_err(|_| EINVAL)?),
                    1 => SeekFrom::Current(off),
                    2 => SeekFrom::End(off),
                    _ => return Err(EINVAL),
                };
                file.seek(from).map_err(|e| errno_of(&e))
            }
            Handle::Dir { pos, entries, .. } => {
                // rewinddir / telldir style positioning.
                let np = seek(if whence == 1 { *pos as i64 } else { 0 }, off)? as usize;
                *pos = np.min(entries.len());
                Ok(*pos as u64)
            }
        }
    }

    pub fn ftruncate(&mut self, fd: i32, len: u64) -> Result<(), u32> {
        let o = self.open.get_mut(&fd).ok_or(EBADF)?;
        match &mut o.handle {
            Handle::Host { file } if o.flags & O_ACCMODE != 0 => file.set_len(len).map_err(|e| errno_of(&e)),
            Handle::Dir { .. } => Err(EISDIR),
            _ => Err(EINVAL),
        }
    }

    pub fn fsync(&mut self, fd: i32) -> Result<(), u32> {
        match &mut self.open.get_mut(&fd).ok_or(EBADF)?.handle {
            Handle::Host { file } => file.sync_all().map_err(|e| errno_of(&e)),
            _ => Ok(()),
        }
    }

    fn stat_target(t: &Target) -> Result<Stat, u32> {
        Ok(match t {
            Target::Virtual(d) => Stat { mode: S_IFREG | 0o444, size: d.len() as u64, mtime: 0, ino: 3, nlink: 1 },
            Target::VirtualDir(_) => Stat { mode: S_IFDIR | 0o755, size: 4096, mtime: 0, ino: 1, nlink: 2 },
            Target::Device(_) => Stat { mode: S_IFCHR | 0o666, size: 0, mtime: 0, ino: 4, nlink: 1 },
            Target::Host { path, .. } => return Self::stat_host(path),
        })
    }

    fn stat_host(path: &std::path::Path) -> Result<Stat, u32> {
        let m = fs::metadata(path).map_err(|e| errno_of(&e))?;
        Ok(Self::stat_meta(&m))
    }

    fn stat_meta(m: &fs::Metadata) -> Stat {
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let perm = if m.permissions().readonly() { 0o444 } else { 0o644 };
        let mode = if m.is_dir() { S_IFDIR | 0o755 } else { S_IFREG | perm };
        Stat { mode, size: m.len(), mtime, ino: 100, nlink: 1 }
    }

    pub fn stat(&self, path: &str) -> Result<Stat, u32> {
        Self::stat_target(&self.resolve(path)?.1)
    }

    pub fn fstat(&self, fd: i32) -> Result<Stat, u32> {
        let o = self.open.get(&fd).ok_or(EBADF)?;
        Ok(match &o.handle {
            Handle::Mem { data, .. } => {
                Stat { mode: S_IFREG | 0o444, size: data.len() as u64, mtime: 0, ino: 3, nlink: 1 }
            }
            Handle::Null | Handle::Zero | Handle::Random => {
                Stat { mode: S_IFCHR | 0o666, size: 0, mtime: 0, ino: 4, nlink: 1 }
            }
            Handle::Host { file } => Self::stat_meta(&file.metadata().map_err(|e| errno_of(&e))?),
            Handle::Dir { .. } => Stat { mode: S_IFDIR | 0o755, size: 4096, mtime: 0, ino: 1, nlink: 2 },
        })
    }

    /// `access(2)`: existence, and write permission only where something is writable.
    pub fn access(&self, path: &str, mode: u32) -> Result<(), u32> {
        let (_, t) = self.resolve(path)?;
        Self::stat_target(&t)?;
        let writable = matches!(t, Target::Host { writable: true, .. } | Target::Device(_));
        if mode & 2 != 0 && !writable {
            return Err(EACCES);
        }
        Ok(())
    }

    fn host_target(&self, path: &str) -> Result<PathBuf, u32> {
        match self.resolve(path)?.1 {
            Target::Host { path, writable: true } => Ok(path),
            Target::Host { .. } => Err(EROFS),
            _ => Err(EACCES),
        }
    }

    pub fn unlink(&mut self, path: &str) -> Result<(), u32> {
        let hp = self.host_target(path)?;
        if fs::metadata(&hp).is_ok_and(|m| m.is_dir()) {
            return Err(EISDIR);
        }
        fs::remove_file(hp).map_err(|e| errno_of(&e))
    }

    pub fn rmdir(&mut self, path: &str) -> Result<(), u32> {
        let hp = self.host_target(path)?;
        fs::remove_dir(hp).map_err(|e| errno_of(&e))
    }

    /// `mkdir(2)`: virtual directories and files already exist; outside the mounts nothing can be created.
    pub fn mkdir(&mut self, path: &str) -> Result<(), u32> {
        match self.resolve(path) {
            Ok((_, Target::Host { path, writable: true })) => fs::create_dir(path).map_err(|e| errno_of(&e)),
            Ok((_, Target::Host { .. })) => Err(EROFS),
            Ok(_) => Err(EEXIST),
            Err(_) => Err(EACCES),
        }
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), u32> {
        let (a, b) = (self.host_target(from)?, self.host_target(to)?);
        fs::rename(a, b).map_err(|e| errno_of(&e))
    }

    /// Next directory entry of an open directory.
    pub fn readdir(&mut self, fd: i32) -> Result<Option<DirEntry>, u32> {
        match &mut self.open.get_mut(&fd).ok_or(EBADF)?.handle {
            Handle::Dir { entries, pos } => {
                let e = entries.get(*pos).cloned();
                *pos += usize::from(e.is_some());
                Ok(e)
            }
            _ => Err(ENOTDIR),
        }
    }

    /// Whole contents of the file at the descriptor's current offset region `[off, off + len)`, zero padded.
    pub fn read_range(&mut self, fd: i32, off: u64, len: usize) -> Result<Vec<u8>, u32> {
        let saved = self.lseek(fd, 0, 1)?;
        self.lseek(fd, off as i64, 0)?;
        let r = self.read(fd, len);
        self.lseek(fd, saved as i64, 0)?;
        let mut data = r?;
        data.resize(len, 0);
        Ok(data)
    }

    /// `readlink`: only `/proc/self/exe` exists.
    pub fn readlink(&self, path: &str) -> Result<String, u32> {
        let p = normalize(&self.cwd, path);
        if p == "/proc/self/exe" || p == format!("/proc/{}/exe", self.pid) {
            return Ok(self.cfg.apk_path());
        }
        match self.resolve(path) {
            Ok(_) => Err(EINVAL),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A fresh scratch directory removed on drop.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "bhpc-vfs-{}-{}",
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

    /// A VFS with `/data/data/pkg` (writable) and `/ro` (read-only) mounted from `tmp`.
    fn vfs_with(tmp: &Tmp) -> Vfs {
        fs::create_dir_all(tmp.path("rw")).unwrap();
        fs::create_dir_all(tmp.path("ro")).unwrap();
        let cfg = FsConfig {
            package: "pkg".into(),
            mounts: vec![
                Mount { guest: "/data/data/pkg".into(), host: tmp.path("rw"), writable: true },
                Mount { guest: "/ro".into(), host: tmp.path("ro"), writable: false },
            ],
        };
        Vfs::new(cfg, 77)
    }

    #[test]
    fn normalize_collapses_dots_and_slashes() {
        assert_eq!(normalize("/", "a//b/./c/../d"), "/a/b/d");
        assert_eq!(normalize("/x/y", "z"), "/x/y/z");
        assert_eq!(normalize("/x", "/../.."), "/");
        assert_eq!(normalize("/", ""), "/");
    }

    #[test]
    fn errno_mapping_prefers_the_os_code_then_the_kind() {
        let raw = std::io::Error::from_raw_os_error(13);
        assert_eq!(errno_of(&raw), 13);
        for (kind, want) in [
            (std::io::ErrorKind::NotFound, ENOENT),
            (std::io::ErrorKind::PermissionDenied, EACCES),
            (std::io::ErrorKind::AlreadyExists, EEXIST),
            (std::io::ErrorKind::InvalidInput, EINVAL),
            (std::io::ErrorKind::Other, EIO),
        ] {
            assert_eq!(errno_of(&std::io::Error::from(kind)), want, "{kind:?}");
        }
    }

    #[test]
    fn config_helpers_describe_the_android_environment() {
        let cfg = FsConfig::default();
        assert_eq!(cfg.package, "com.noodlecake.blockheads");
        assert_eq!(cfg.apk_path(), "/data/app/com.noodlecake.blockheads-1.apk");
        let tmp = Tmp::new();
        let cfg = cfg.with_data_root(&tmp.0).with_apk(std::path::Path::new("/some/game.apk"));
        assert_eq!(cfg.mounts.len(), 3);
        assert_eq!(cfg.mounts[2], Mount { guest: cfg.apk_path(), host: "/some/game.apk".into(), writable: false });
        assert!(cfg.data_dirs().contains(&"sdcard/Android/obb/com.noodlecake.blockheads".to_string()));
        let env = cfg.env();
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("PACKAGE_NAME").unwrap(), "com.noodlecake.blockheads");
        assert_eq!(get("PACKAGE_CODE_PATH").unwrap(), cfg.apk_path());
        assert_eq!(get("CFProcessPath").unwrap(), format!("{}/blockheads", cfg.apk_path()));
        assert_eq!(get("HOME").unwrap(), "/data/data/com.noodlecake.blockheads");
        assert_eq!(get("TZ").unwrap(), "UTC");
        // A package without dots still yields a process name.
        let single = FsConfig { package: "solo".into(), mounts: vec![] };
        assert!(single.env().iter().any(|(k, v)| k == "CFProcessPath" && v.ends_with("/solo")));
    }

    #[test]
    fn virtual_files_and_directories() {
        let mut v = Vfs::new(FsConfig::default(), 4242);
        let fd = v.open("/proc/self/cmdline", 0).unwrap();
        assert_eq!(v.read(fd, 100).unwrap(), b"com.noodlecake.blockheads\0");
        assert_eq!(v.read(fd, 10).unwrap(), b"", "at end of file");
        assert_eq!(v.lseek(fd, 4, 0).unwrap(), 4);
        assert_eq!(v.lseek(fd, 2, 1).unwrap(), 6);
        assert_eq!(v.lseek(fd, -1, 2).unwrap(), 25);
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
        assert_eq!(v.open("/proc/cpuinfo", O_WRONLY), Err(EACCES));
        assert_eq!(v.open("/proc/cpuinfo", O_CREAT | O_EXCL), Err(EEXIST));
        assert_eq!(v.open("/proc/cpuinfo", O_DIRECTORY), Err(ENOTDIR));
        assert_eq!(v.open("/proc", O_WRONLY), Err(EISDIR));
        assert_eq!(v.open("", 0), Err(ENOENT));
        assert_eq!(v.open("/nowhere", 0), Err(ENOENT));
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
            assert_eq!(v.read(fd, 1), Err(EISDIR));
            assert_eq!(v.write(fd, b"x"), Err(EBADF), "directories are read-only handles");
            assert_eq!(v.ftruncate(fd, 0), Err(EISDIR));
            assert_eq!(v.fstat(fd).unwrap().mode, S_IFDIR | 0o755);
        }
        let d = v.open("/data/data", 0).unwrap();
        assert_eq!(v.readdir(d).unwrap().unwrap().name, ".");
        let f = v.open("/proc/cpuinfo", 0).unwrap();
        assert_eq!(v.readdir(f), Err(ENOTDIR));
        assert_eq!(v.readdir(999), Err(EBADF));
    }

    #[test]
    fn devices() {
        let mut v = Vfs::new(FsConfig::default(), 1);
        let null = v.open("/dev/null", O_RDWR).unwrap();
        assert_eq!(v.write(null, b"abc").unwrap(), 3);
        assert_eq!(v.read(null, 8).unwrap(), b"");
        assert_eq!(v.lseek(null, 5, 0).unwrap(), 0);
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
        assert_eq!(v.fstat(null).unwrap().mode, S_IFCHR | 0o666);
        assert_eq!(v.stat("/dev/zero").unwrap().mode, S_IFCHR | 0o666);
        assert!(v.access("/dev/null", 2).is_ok());
        assert_eq!(v.ftruncate(null, 0), Err(EINVAL));
        v.fsync(null).unwrap();
        assert_eq!(v.fsync(77), Err(EBADF));
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
        assert_eq!(v.ftruncate(fd, 0), Err(EINVAL), "read-only descriptor");
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
        assert_eq!(v.mkdir("/proc/cpuinfo"), Err(EEXIST));
        assert_eq!(v.mkdir("/ro/x"), Err(EROFS));
        assert_eq!(v.mkdir("/elsewhere/x"), Err(EACCES));
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
        assert_eq!(v.rename("/data/data/pkg/sub/c", "/ro/c"), Err(EROFS));
        v.unlink("/data/data/pkg/sub/c").unwrap();
        v.unlink("/data/data/pkg/sub/b").unwrap();
        assert_eq!(v.unlink("/data/data/pkg/sub/b"), Err(ENOENT));
        v.rmdir("/data/data/pkg/sub").unwrap();
        assert_eq!(v.unlink("/proc/cpuinfo"), Err(EACCES));
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
        assert_eq!(v.stat("/proc").unwrap().mode, S_IFDIR | 0o755);
        assert_eq!(v.stat("/proc/cpuinfo").unwrap().mode, S_IFREG | 0o444);
        assert_eq!(v.stat("/none"), Err(ENOENT));
        // Read-only mount.
        fs::write(tmp.path("ro/r.txt"), b"readonly").unwrap();
        assert_eq!(v.open("/ro/r.txt", O_WRONLY), Err(EROFS));
        assert_eq!(v.open("/ro/r.txt", O_CREAT), Err(EROFS));
        let fd = v.open("/ro/r.txt", 0).unwrap();
        assert_eq!(v.read(fd, 100).unwrap(), b"readonly");
        let mut perm = fs::metadata(tmp.path("ro/r.txt")).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(tmp.path("ro/r.txt"), perm).unwrap();
        assert_eq!(v.stat("/ro/r.txt").unwrap().mode, S_IFREG | 0o444);
        // Mounts match whole path components, and intermediate directories exist.
        assert_eq!(v.open("/ro2/x", 0), Err(ENOENT));
        let cfg = FsConfig {
            package: "p".into(),
            mounts: vec![Mount { guest: "/a/b/c".into(), host: tmp.path("rw"), writable: true }],
        };
        let w = Vfs::new(cfg, 1);
        assert_eq!(w.stat("/a/b").unwrap().mode, S_IFDIR | 0o755);
        assert_eq!(w.stat("/a/bc"), Err(ENOENT));
    }

    #[test]
    fn readlink_and_descriptor_limits() {
        let mut v = Vfs::new(FsConfig::default(), 9);
        assert_eq!(v.readlink("/proc/self/exe").unwrap(), "/data/app/com.noodlecake.blockheads-1.apk");
        assert!(v.readlink("/proc/9/exe").is_ok());
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
        assert_eq!(v.config().package, "com.noodlecake.blockheads");
    }

    #[test]
    fn path_tracing_is_optional() {
        let mut v = Vfs::new(FsConfig::default(), 1);
        v.trace = true;
        assert!(v.stat("/proc/cpuinfo").is_ok());
    }

    #[test]
    fn host_io_errors_are_translated() {
        use std::os::fd::FromRawFd;
        let tmp = Tmp::new();
        let mut v = vfs_with(&tmp);
        let dir_file = || fs::File::open(&tmp.0).unwrap();
        // Reading a directory through a plain file handle fails with EISDIR.
        v.open.insert(10, Open { handle: Handle::Host { file: dir_file() }, flags: 0 });
        assert_eq!(v.read(10, 4), Err(EISDIR));
        // Writing through a read-only OS handle, truncating it, syncing a pipe, seeking a pipe.
        fs::write(tmp.path("rw/ro"), b"abc").unwrap();
        v.open.insert(
            11,
            Open { handle: Handle::Host { file: fs::File::open(tmp.path("rw/ro")).unwrap() }, flags: O_RDWR },
        );
        assert!(v.write(11, b"x").is_err());
        assert!(v.ftruncate(11, 0).is_err());
        let mut fds = [0i32; 2];
        // SAFETY: plain pipe(2) call with a two-element buffer.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: the descriptors were just created and are owned by these files.
        let (r, w) = unsafe { (fs::File::from_raw_fd(fds[0]), fs::File::from_raw_fd(fds[1])) };
        v.open.insert(12, Open { handle: Handle::Host { file: r }, flags: 0 });
        v.open.insert(13, Open { handle: Handle::Host { file: w }, flags: O_WRONLY });
        assert_eq!(v.lseek(12, 0, 0), Err(ESPIPE));
        assert!(v.fsync(12).is_err());
        // A descriptor that was closed behind our back cannot be inspected.
        // SAFETY: fd 900 is not open; the File is dropped immediately and closing it merely fails.
        let stale = unsafe { fs::File::from_raw_fd(900) };
        v.open.insert(14, Open { handle: Handle::Host { file: stale }, flags: 0 });
        assert_eq!(v.fstat(14), Err(EBADF));
        // Dropping the stale file would abort (std checks for double closes): leak it instead.
        std::mem::forget(v.open.remove(&14));
        // Writing to memory-backed and directory handles that claim to be writable.
        v.open.insert(15, Open { handle: Handle::Mem { data: vec![1], pos: 0 }, flags: O_RDWR });
        assert_eq!(v.write(15, b"x"), Err(EBADF));
        v.open.insert(16, Open { handle: Handle::Dir { entries: vec![], pos: 0 }, flags: O_RDWR });
        assert_eq!(v.write(16, b"x"), Err(EISDIR));
        // Listing a directory that vanished.
        assert_eq!(v.host_dir(&tmp.path("missing")), Err(ENOENT));
        fs::create_dir(tmp.path("rw/full")).unwrap();
        fs::write(tmp.path("rw/full/x"), b"").unwrap();
        assert_eq!(v.rmdir("/data/data/pkg/full"), Err(ENOTEMPTY));
    }
}
