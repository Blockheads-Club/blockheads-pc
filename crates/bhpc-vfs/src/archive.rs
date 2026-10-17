//! Lazy, zip-backed view of the user's APK.
//!
//! Only the central directory is read when the archive is opened. An entry is decompressed the
//! first time somebody opens it; concurrent opens of the same entry share one buffer for as long
//! as any of them is alive (the archive keeps only weak references, so memory is not pinned).
//! Nothing is ever written to disk.

use crate::error::{Result, VfsError};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

/// Anything a zip can be read from.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Directory index: child name to "is a directory".
type Children = BTreeMap<String, bool>;

/// A read-only zip archive with a file/directory index and a weak decompression cache.
pub struct ApkArchive {
    zip: Mutex<zip::ZipArchive<Box<dyn ReadSeek>>>,
    /// Entry name to (zip index, uncompressed size).
    files: HashMap<String, (usize, u64)>,
    /// Directory path (no trailing slash, `""` is the root) to its children.
    dirs: HashMap<String, Children>,
    cache: Mutex<HashMap<String, Weak<[u8]>>>,
}

impl ApkArchive {
    /// Opens the APK at `path`; the file stays open and is read on demand.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(File::open(path)?)
    }

    /// Opens an archive held in memory (tests, embedded fixtures).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::from_reader(Cursor::new(bytes))
    }

    /// Opens an archive from any seekable reader.
    pub fn from_reader<R: Read + Seek + Send + 'static>(reader: R) -> Result<Self> {
        let boxed: Box<dyn ReadSeek> = Box::new(reader);
        let mut zip = zip::ZipArchive::new(boxed)?;
        let mut files = HashMap::new();
        let mut dirs: HashMap<String, Children> = HashMap::new();
        dirs.entry(String::new()).or_default();
        for i in 0..zip.len() {
            let entry = zip.by_index_raw(i)?;
            let name = entry.name().trim_start_matches('/').to_string();
            let is_dir = entry.is_dir();
            let size = entry.size();
            drop(entry);
            let trimmed = name.trim_end_matches('/');
            if trimmed.is_empty() {
                continue;
            }
            // Register every parent directory and the leaf itself.
            let mut parent = String::new();
            let parts: Vec<&str> = trimmed.split('/').collect();
            for (n, part) in parts.iter().enumerate() {
                let leaf = n + 1 == parts.len();
                let child_is_dir = !leaf || is_dir;
                dirs.entry(parent.clone()).or_default().insert((*part).to_string(), child_is_dir);
                if parent.is_empty() {
                    parent = (*part).to_string();
                } else {
                    parent = format!("{parent}/{part}");
                }
                if child_is_dir {
                    dirs.entry(parent.clone()).or_default();
                }
            }
            if !is_dir {
                files.insert(trimmed.to_string(), (i, size));
            }
        }
        Ok(ApkArchive { zip: Mutex::new(zip), files, dirs, cache: Mutex::new(HashMap::new()) })
    }

    /// Number of file entries.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// All file entry names, sorted.
    pub fn file_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.files.keys().cloned().collect();
        v.sort();
        v
    }

    /// Size of a file entry without decompressing it.
    pub fn file_size(&self, name: &str) -> Option<u64> {
        self.files.get(name.trim_matches('/')).map(|f| f.1)
    }

    /// True when `name` is a (possibly implicit) directory; `""` is the root.
    pub fn is_dir(&self, name: &str) -> bool {
        self.dirs.contains_key(name.trim_matches('/'))
    }

    /// True when `name` is a file entry.
    pub fn is_file(&self, name: &str) -> bool {
        self.files.contains_key(name.trim_matches('/'))
    }

    /// Children of a directory as (name, is_dir), sorted by name.
    pub fn list(&self, dir: &str) -> Option<Vec<(String, bool)>> {
        self.dirs.get(dir.trim_matches('/')).map(|c| c.iter().map(|(n, d)| (n.clone(), *d)).collect())
    }

    /// Returns the decompressed content of a file entry, sharing a live buffer if one exists.
    pub fn read(&self, name: &str) -> Result<Arc<[u8]>> {
        let name = name.trim_matches('/');
        let &(idx, size) = self.files.get(name).ok_or(VfsError::NotFound)?;
        let mut cache = self.cache.lock().unwrap();
        if let Some(live) = cache.get(name).and_then(Weak::upgrade) {
            return Ok(live);
        }
        let mut buf = Vec::with_capacity(size as usize);
        {
            let mut zip = self.zip.lock().unwrap();
            let mut entry = zip.by_index(idx)?;
            entry.read_to_end(&mut buf).map_err(|_| VfsError::Io)?;
        }
        let data: Arc<[u8]> = buf.into();
        cache.retain(|_, w| w.strong_count() > 0);
        cache.insert(name.to_string(), Arc::downgrade(&data));
        Ok(data)
    }
}
