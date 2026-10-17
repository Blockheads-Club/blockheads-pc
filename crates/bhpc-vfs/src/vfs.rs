//! The mount table: every guest path goes through here.

use crate::error::{Result, VfsError};
use crate::fs::{DirEntry, FileKind, FileSystem, OpenFlags, Stat, VFile};
use crate::path::{normalize, strip_mount};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};

/// Upper bound of remembered missing paths (so a probing guest cannot grow memory forever).
const MAX_MISSES: usize = 4096;

struct Mount {
    prefix: String,
    fs: Arc<dyn FileSystem>,
}

/// A table of mount points with longest-prefix dispatch.
///
/// * Paths are normalised first (relative ones against the current directory).
/// * The deepest mount containing the path serves it; mount points shadow what the parent
///   backend may hold at the same name.
/// * Parents of mount points exist as virtual directories even when no backend has them, and
///   directory listings merge the backend's entries with the mount points below.
/// * `ENOENT` results are recorded (see [`Vfs::take_misses`]) so unmapped guest paths are visible.
pub struct Vfs {
    mounts: RwLock<Vec<Mount>>,
    cwd: Mutex<String>,
    misses: Mutex<BTreeSet<String>>,
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs {
    /// An empty table with the current directory at `/`.
    pub fn new() -> Self {
        Vfs { mounts: RwLock::new(Vec::new()), cwd: Mutex::new("/".to_string()), misses: Mutex::new(BTreeSet::new()) }
    }

    /// Mounts `fs` at `prefix`, replacing an existing mount at the same point.
    pub fn mount(&self, prefix: &str, fs: Arc<dyn FileSystem>) -> Result<()> {
        let prefix = normalize("/", prefix)?;
        let mut mounts = self.mounts.write().unwrap();
        mounts.retain(|m| m.prefix != prefix);
        mounts.push(Mount { prefix, fs });
        Ok(())
    }

    /// Mount points, sorted.
    pub fn mount_points(&self) -> Vec<String> {
        let mut v: Vec<String> = self.mounts.read().unwrap().iter().map(|m| m.prefix.clone()).collect();
        v.sort();
        v
    }

    /// Current working directory (`getcwd`).
    pub fn cwd(&self) -> String {
        self.cwd.lock().unwrap().clone()
    }

    /// `chdir`: the target must be a directory.
    pub fn chdir(&self, path: &str) -> Result<()> {
        let abs = self.resolve(path)?;
        if self.stat(&abs)?.kind != FileKind::Dir {
            return Err(VfsError::NotDir);
        }
        *self.cwd.lock().unwrap() = abs;
        Ok(())
    }

    /// Normalises a guest path against the current directory.
    pub fn resolve(&self, path: &str) -> Result<String> {
        normalize(&self.cwd.lock().unwrap(), path)
    }

    /// Paths that were looked up and not found since the last call, sorted.
    pub fn take_misses(&self) -> Vec<String> {
        std::mem::take(&mut *self.misses.lock().unwrap()).into_iter().collect()
    }

    fn miss<T>(&self, op: &str, abs: &str, r: Result<T>) -> Result<T> {
        if matches!(r, Err(VfsError::NotFound)) {
            let mut m = self.misses.lock().unwrap();
            if m.len() < MAX_MISSES {
                m.insert(format!("{op} {abs}"));
            }
        }
        r
    }

    /// Deepest mount for `abs`: (mount index, backend, path inside the backend).
    fn find(&self, abs: &str) -> Option<(usize, Arc<dyn FileSystem>, String)> {
        let mounts = self.mounts.read().unwrap();
        mounts
            .iter()
            .enumerate()
            .filter_map(|(i, m)| strip_mount(abs, &m.prefix).map(|rest| (i, m, rest)))
            .max_by_key(|(_, m, _)| m.prefix.len())
            .map(|(i, m, rest)| (i, m.fs.clone(), rest.to_string()))
    }

    /// Names of mount points that lie strictly below `abs`, with the backend when the entry is
    /// the mount point itself: name to (is direct mount point -> Some(fs)).
    fn children_of(&self, abs: &str) -> BTreeMap<String, Option<Arc<dyn FileSystem>>> {
        let mut out = BTreeMap::new();
        let base = if abs == "/" { "" } else { abs };
        for m in self.mounts.read().unwrap().iter() {
            let Some(rest) = m.prefix.strip_prefix(base).and_then(|r| r.strip_prefix('/')) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            match rest.split_once('/') {
                Some((first, _)) => {
                    out.entry(first.to_string()).or_insert(None);
                }
                None => {
                    out.insert(rest.to_string(), Some(m.fs.clone()));
                }
            }
        }
        out
    }

    fn is_virtual_dir(&self, abs: &str) -> bool {
        !self.children_of(abs).is_empty()
    }

    fn denied(&self, abs: &str) -> VfsError {
        if self.is_virtual_dir(abs) {
            VfsError::PermissionDenied
        } else {
            VfsError::NotFound
        }
    }
}

impl FileSystem for Vfs {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>> {
        let abs = self.resolve(path)?;
        let r = match self.find(&abs) {
            Some((_, fs, rest)) => match fs.open(&rest, flags) {
                Err(VfsError::NotFound) if self.is_virtual_dir(&abs) => Err(VfsError::IsDir),
                r => r,
            },
            None if self.is_virtual_dir(&abs) => Err(VfsError::IsDir),
            None => Err(VfsError::NotFound),
        };
        self.miss("open", &abs, r)
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        let abs = self.resolve(path)?;
        let r = match self.find(&abs) {
            Some((_, fs, rest)) => match fs.stat(&rest) {
                Err(VfsError::NotFound) if self.is_virtual_dir(&abs) => Ok(Stat::dir()),
                r => r,
            },
            None if self.is_virtual_dir(&abs) => Ok(Stat::dir()),
            None => Err(VfsError::NotFound),
        };
        self.miss("stat", &abs, r)
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let abs = self.resolve(path)?;
        let below = self.children_of(&abs);
        let base = match self.find(&abs) {
            Some((_, fs, rest)) => match fs.read_dir(&rest) {
                Err(VfsError::NotFound) if !below.is_empty() => Ok(Vec::new()),
                r => r,
            },
            None if !below.is_empty() => Ok(Vec::new()),
            None => Err(VfsError::NotFound),
        };
        let r = base.map(|entries| {
            let mut map: BTreeMap<String, FileKind> = entries.into_iter().map(|e| (e.name, e.kind)).collect();
            for (name, mount) in below {
                let kind = mount.and_then(|fs| fs.stat("/").ok()).map_or(FileKind::Dir, |s| s.kind);
                map.insert(name, kind);
            }
            map.into_iter().map(|(name, kind)| DirEntry { name, kind }).collect()
        });
        self.miss("readdir", &abs, r)
    }

    fn mkdir(&self, path: &str) -> Result<()> {
        let abs = self.resolve(path)?;
        let r = match self.find(&abs) {
            // A read-only backend refuses everything, but an existing path is `EEXIST` first.
            Some((_, fs, rest)) => match fs.mkdir(&rest) {
                Err(VfsError::ReadOnly) if self.stat(&abs).is_ok() => Err(VfsError::Exists),
                r => r,
            },
            None if self.is_virtual_dir(&abs) => Err(VfsError::Exists),
            None => Err(VfsError::NotFound),
        };
        self.miss("mkdir", &abs, r)
    }

    fn unlink(&self, path: &str) -> Result<()> {
        let abs = self.resolve(path)?;
        let r = match self.find(&abs) {
            Some((_, fs, rest)) => match fs.unlink(&rest) {
                Err(VfsError::ReadOnly) if self.stat(&abs) == Err(VfsError::NotFound) => Err(VfsError::NotFound),
                r => r,
            },
            None => Err(self.denied(&abs)),
        };
        self.miss("unlink", &abs, r)
    }

    fn rmdir(&self, path: &str) -> Result<()> {
        let abs = self.resolve(path)?;
        let r = match self.find(&abs) {
            Some((_, fs, rest)) => match fs.rmdir(&rest) {
                Err(VfsError::ReadOnly) if self.stat(&abs) == Err(VfsError::NotFound) => Err(VfsError::NotFound),
                r => r,
            },
            None => Err(self.denied(&abs)),
        };
        self.miss("rmdir", &abs, r)
    }

    fn readlink(&self, path: &str) -> Result<String> {
        let abs = self.resolve(path)?;
        let r = match self.find(&abs) {
            Some((_, fs, rest)) => match fs.readlink(&rest) {
                Err(VfsError::Invalid) => self.stat(&abs).and(Err(VfsError::Invalid)),
                r => r,
            },
            None => self.stat(&abs).and(Err(VfsError::Invalid)),
        };
        self.miss("readlink", &abs, r)
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (a, b) = (self.resolve(from)?, self.resolve(to)?);
        let r = match (self.find(&a), self.find(&b)) {
            (Some((i, fs, ra)), Some((j, _, rb))) if i == j => fs.rename(&ra, &rb),
            (Some(_), Some(_)) => Err(VfsError::CrossDevice),
            (None, _) => Err(self.denied(&a)),
            (_, None) => Err(self.denied(&b)),
        };
        self.miss("rename", &a, r)
    }
}
