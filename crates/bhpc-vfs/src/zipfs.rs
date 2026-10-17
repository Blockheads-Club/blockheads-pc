//! A read-only [`FileSystem`] over a sub-tree of the APK.

use crate::archive::ApkArchive;
use crate::asset::Asset;
use crate::error::{Result, VfsError};
use crate::fs::{DirEntry, FileKind, FileSystem, OpenFlags, Stat, VFile};
use std::sync::Arc;

/// Exposes `<root>/...` of an archive as `/...`.
pub struct ZipFs {
    apk: Arc<ApkArchive>,
    root: String,
}

impl ZipFs {
    /// `root` is a zip prefix such as `"assets"` or `"com.apportable.system"` (`""` for all).
    pub fn new(apk: Arc<ApkArchive>, root: &str) -> Self {
        ZipFs { apk, root: root.trim_matches('/').to_string() }
    }

    fn entry(&self, path: &str) -> String {
        let rest = path.trim_matches('/');
        match (self.root.is_empty(), rest.is_empty()) {
            (true, _) => rest.to_string(),
            (false, true) => self.root.clone(),
            (false, false) => format!("{}/{}", self.root, rest),
        }
    }
}

impl FileSystem for ZipFs {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>> {
        let name = self.entry(path);
        if flags.write || flags.create || flags.truncate || flags.append {
            return Err(VfsError::ReadOnly);
        }
        if self.apk.is_dir(&name) {
            return Err(VfsError::IsDir);
        }
        if flags.directory {
            return Err(if self.apk.is_file(&name) { VfsError::NotDir } else { VfsError::NotFound });
        }
        Ok(Box::new(Asset::new(self.apk.read(&name)?)))
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        let name = self.entry(path);
        if let Some(size) = self.apk.file_size(&name) {
            Ok(Stat::file(size))
        } else if self.apk.is_dir(&name) {
            Ok(Stat::dir())
        } else {
            Err(VfsError::NotFound)
        }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let name = self.entry(path);
        match self.apk.list(&name) {
            Some(children) => Ok(children
                .into_iter()
                .map(|(name, d)| DirEntry { name, kind: if d { FileKind::Dir } else { FileKind::File } })
                .collect()),
            None if self.apk.is_file(&name) => Err(VfsError::NotDir),
            None => Err(VfsError::NotFound),
        }
    }
}
