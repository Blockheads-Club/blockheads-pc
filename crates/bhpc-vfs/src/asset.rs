//! AAsset-style API over the APK's `assets/` tree, for the JNI `AssetManager`.
//!
//! The semantics follow `<android/asset_manager.h>`: [`Asset::read`] returns 0 at the end,
//! [`Asset::seek`] rejects positions outside `0..=length`, [`Asset::buffer`] hands out the whole
//! content, and [`AssetDir`] enumerates *files* of one directory (directories are skipped, as
//! `AAssetDir_getNextFileName` does). Java's `AssetManager.list()` includes directories, which
//! [`AssetManager::list`] reproduces.

use crate::archive::ApkArchive;
use crate::error::{Result, VfsError};
use crate::fs::{seek_in, VFile};
use std::io::SeekFrom;
use std::sync::Arc;

/// `whence` of `AAsset_seek`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whence {
    /// `SEEK_SET`
    Set,
    /// `SEEK_CUR`
    Cur,
    /// `SEEK_END`
    End,
}

impl Whence {
    /// Converts a C `whence` (0, 1, 2).
    pub fn from_raw(w: i32) -> Result<Self> {
        match w {
            0 => Ok(Whence::Set),
            1 => Ok(Whence::Cur),
            2 => Ok(Whence::End),
            _ => Err(VfsError::Invalid),
        }
    }
}

/// An open asset: a cursor over one shared, fully decompressed buffer.
#[derive(Debug, Clone)]
pub struct Asset {
    data: Arc<[u8]>,
    /// May exceed the length after a POSIX-style seek past the end.
    pos: u64,
}

impl Asset {
    /// Wraps a buffer.
    pub fn new(data: Arc<[u8]>) -> Self {
        Asset { data, pos: 0 }
    }

    /// `AAsset_getLength`.
    pub fn length(&self) -> u64 {
        self.data.len() as u64
    }

    /// `AAsset_getRemainingLength`.
    pub fn remaining(&self) -> u64 {
        self.length().saturating_sub(self.pos)
    }

    /// `AAsset_getBuffer`: the whole content, independent of the position.
    pub fn buffer(&self) -> &[u8] {
        &self.data
    }

    /// `AAsset_read`: copies up to `buf.len()` bytes and advances; 0 at the end.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let start = self.pos.min(self.length()) as usize;
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        if n > 0 {
            self.pos = (start + n) as u64;
        }
        n
    }

    /// `AAsset_seek`: returns the new position, `EINVAL` when it would leave `0..=length`.
    pub fn seek(&mut self, offset: i64, whence: Whence) -> Result<u64> {
        let base = match whence {
            Whence::Set => 0,
            Whence::Cur => self.pos as i128,
            Whence::End => self.length() as i128,
        };
        let new = base + offset as i128;
        if new < 0 || new > self.length() as i128 {
            return Err(VfsError::Invalid);
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

/// `VFile` view, so bionic `open()` on an asset path shares this implementation. Unlike
/// `AAsset_seek`, POSIX allows seeking past the end.
impl VFile for Asset {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(Asset::read(self, buf))
    }
    fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        Err(VfsError::BadFd)
    }
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.pos = seek_in(self.length(), self.pos, pos)?;
        Ok(self.pos)
    }
    fn len(&mut self) -> Result<u64> {
        Ok(self.length())
    }
    fn set_len(&mut self, _len: u64) -> Result<()> {
        Err(VfsError::BadFd)
    }
    fn sync(&mut self) -> Result<()> {
        Ok(())
    }
}

/// `AAssetDir`: the files of one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDir {
    names: Vec<String>,
    next: usize,
}

impl AssetDir {
    /// `AAssetDir_getNextFileName`.
    pub fn next_file_name(&mut self) -> Option<&str> {
        let n = self.names.get(self.next)?;
        self.next += 1;
        Some(n)
    }

    /// `AAssetDir_rewind`.
    pub fn rewind(&mut self) {
        self.next = 0;
    }

    /// All file names.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// `AAssetManager` over one root inside the APK (normally `assets/`).
#[derive(Clone)]
pub struct AssetManager {
    apk: Arc<ApkArchive>,
    root: String,
}

impl AssetManager {
    /// `root` is the zip prefix that asset names are relative to, e.g. `"assets"`.
    pub fn new(apk: Arc<ApkArchive>, root: &str) -> Self {
        AssetManager { apk, root: root.trim_matches('/').to_string() }
    }

    /// The standard Android manager: names relative to `assets/`.
    pub fn android(apk: Arc<ApkArchive>) -> Self {
        Self::new(apk, "assets")
    }

    fn full(&self, name: &str) -> String {
        let name = name.trim_matches('/');
        match (self.root.is_empty(), name.is_empty()) {
            (true, _) => name.to_string(),
            (false, true) => self.root.clone(),
            (false, false) => format!("{}/{}", self.root, name),
        }
    }

    /// The underlying archive.
    pub fn archive(&self) -> &Arc<ApkArchive> {
        &self.apk
    }

    /// `AAssetManager_open`.
    pub fn open(&self, name: &str) -> Result<Asset> {
        Ok(Asset::new(self.apk.read(&self.full(name))?))
    }

    /// True when the asset (file) exists.
    pub fn exists(&self, name: &str) -> bool {
        self.apk.is_file(&self.full(name))
    }

    /// Length of an asset without reading it.
    pub fn length(&self, name: &str) -> Option<u64> {
        self.apk.file_size(&self.full(name))
    }

    /// `AAssetManager_openDir`: the files directly inside `dir` (`""` is the root).
    pub fn open_dir(&self, dir: &str) -> Result<AssetDir> {
        let names = self.list(dir)?.into_iter().filter(|(_, is_dir)| !is_dir).map(|(n, _)| n).collect();
        Ok(AssetDir { names, next: 0 })
    }

    /// Java `AssetManager.list()`: files and directories, sorted.
    pub fn list(&self, dir: &str) -> Result<Vec<(String, bool)>> {
        self.apk.list(&self.full(dir)).ok_or(VfsError::NotFound)
    }
}
