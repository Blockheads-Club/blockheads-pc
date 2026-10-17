//! Read-only, in-memory APK (zip) reader and known-build identification.
//!
//! Only what an APK needs: stored and deflated entries, no zip64, no encryption. Nothing is ever written
//! to disk; libraries are decompressed into memory on demand.

use crate::{LibSource, LoadError};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;

/// Directory of the ARM32 libraries inside the APK.
pub const LIB_DIR: &str = "lib/armeabi-v7a/";
/// The library whose hash identifies the build.
pub const IDENTITY_LIB: &str = "libApplication.so";

/// A known Blockheads release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// 1.7.4
    V1_7_4,
    /// 1.7.5
    V1_7_5,
    /// 1.7.6
    V1_7_6,
}

impl Release {
    /// Version string.
    pub fn version(self) -> &'static str {
        match self {
            Release::V1_7_4 => "1.7.4",
            Release::V1_7_5 => "1.7.5",
            Release::V1_7_6 => "1.7.6",
        }
    }
}

/// SHA-256 (lowercase hex) of `lib/armeabi-v7a/libApplication.so` for each known release.
pub const KNOWN_BUILDS: [(Release, &str); 3] = [
    (Release::V1_7_4, "e40f53436fe3028aefaa678f2312aab9d2a9a024747aa797d6334494cf225861"),
    (Release::V1_7_5, "d09418e9c0865902054a71358dcff3264d47f7b24ede58667cb5ea0e6f269b96"),
    (Release::V1_7_6, "733d821027d69de329d0ba171df2e6013d612edf5a4d327badd001acc30b94c7"),
];

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// The release whose `libApplication.so` hashes to `sha256_hex`.
pub fn identify(sha256_hex: &str) -> Option<Release> {
    KNOWN_BUILDS.iter().find(|(_, h)| *h == sha256_hex).map(|(r, _)| *r)
}

/// APK reader failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApkError {
    /// Host I/O failure while reading the file.
    Io(String),
    /// No end-of-central-directory record.
    NotZip,
    /// A record points outside the file.
    Truncated,
    /// A zip feature we do not implement (zip64, encryption, exotic compression).
    Unsupported(&'static str),
    /// Inconsistent zip structures or entry data.
    Corrupt(String),
    /// No such entry.
    Missing(String),
    /// `libApplication.so` is not one of the known builds (and `force` was not given).
    UnknownBuild {
        /// Lowercase hex SHA-256 of the library that was found.
        sha256: String,
    },
}

impl fmt::Display for ApkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApkError::Io(e) => write!(f, "cannot read APK: {e}"),
            ApkError::NotZip => write!(f, "not a zip/APK file"),
            ApkError::Truncated => write!(f, "truncated APK"),
            ApkError::Unsupported(w) => write!(f, "unsupported zip feature: {w}"),
            ApkError::Corrupt(w) => write!(f, "corrupt APK: {w}"),
            ApkError::Missing(n) => write!(f, "APK has no entry {n}"),
            ApkError::UnknownBuild { sha256 } => {
                write!(f, "unknown Blockheads build (libApplication.so sha256 {sha256}); use --force to try anyway")
            }
        }
    }
}

impl std::error::Error for ApkError {}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    crc: u32,
    csize: u32,
    usize_: u32,
    local_header: u32,
}

/// An opened APK held in memory.
pub struct Apk {
    data: Vec<u8>,
    entries: Vec<Entry>,
}

fn u16le(b: &[u8], o: usize) -> Result<u16, ApkError> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(ApkError::Truncated)
}

fn u32le(b: &[u8], o: usize) -> Result<u32, ApkError> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(ApkError::Truncated)
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    !data.iter().fold(!0u32, |c, b| table[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8))
}

impl Apk {
    /// Read the whole file into memory and parse its central directory.
    pub fn open(path: &Path) -> Result<Apk, ApkError> {
        Apk::from_bytes(std::fs::read(path).map_err(|e| ApkError::Io(format!("{}: {e}", path.display())))?)
    }

    /// Parse an APK already in memory.
    pub fn from_bytes(data: Vec<u8>) -> Result<Apk, ApkError> {
        let eocd = (0..=data.len().saturating_sub(22))
            .rev()
            .take(22 + 0xffff)
            .find(|&i| data.get(i..i + 4) == Some(b"PK\x05\x06"))
            .ok_or(ApkError::NotZip)?;
        let total = u16le(&data, eocd + 10)? as usize;
        let cd_size = u32le(&data, eocd + 12)? as usize;
        let cd_off = u32le(&data, eocd + 16)? as usize;
        if total == 0xffff || cd_off == 0xffff_ffff {
            return Err(ApkError::Unsupported("zip64"));
        }
        if cd_off.checked_add(cd_size).is_none_or(|e| e > data.len()) {
            return Err(ApkError::Truncated);
        }
        let mut entries = Vec::with_capacity(total);
        let mut o = cd_off;
        for _ in 0..total {
            if data.get(o..o + 4) != Some(b"PK\x01\x02") {
                return Err(ApkError::Corrupt("bad central directory entry".into()));
            }
            let flags = u16le(&data, o + 8)?;
            let nlen = u16le(&data, o + 28)? as usize;
            let elen = u16le(&data, o + 30)? as usize;
            let clen = u16le(&data, o + 32)? as usize;
            let name = data.get(o + 46..o + 46 + nlen).ok_or(ApkError::Truncated)?;
            let e = Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                method: u16le(&data, o + 10)?,
                crc: u32le(&data, o + 16)?,
                csize: u32le(&data, o + 20)?,
                usize_: u32le(&data, o + 24)?,
                local_header: u32le(&data, o + 42)?,
            };
            if flags & 1 != 0 {
                return Err(ApkError::Unsupported("encryption"));
            }
            if e.csize == 0xffff_ffff || e.usize_ == 0xffff_ffff {
                return Err(ApkError::Unsupported("zip64"));
            }
            entries.push(e);
            o += 46 + nlen + elen + clen;
        }
        Ok(Apk { data, entries })
    }

    /// All entry names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    /// File names (without directory) of `lib/armeabi-v7a/*.so`, in archive order.
    pub fn libs(&self) -> Vec<&str> {
        self.names()
            .filter_map(|n| n.strip_prefix(LIB_DIR))
            .filter(|n| n.ends_with(".so") && !n.contains('/'))
            .collect()
    }

    /// Decompress the entry `path` (checks size and CRC-32).
    pub fn read(&self, path: &str) -> Result<Vec<u8>, ApkError> {
        let e = self.entries.iter().find(|e| e.name == path).ok_or_else(|| ApkError::Missing(path.to_string()))?;
        let lh = e.local_header as usize;
        if self.data.get(lh..lh + 4) != Some(b"PK\x03\x04") {
            return Err(ApkError::Corrupt(format!("bad local header for {path}")));
        }
        let start = lh + 30 + u16le(&self.data, lh + 26)? as usize + u16le(&self.data, lh + 28)? as usize;
        let raw = self.data.get(start..start + e.csize as usize).ok_or(ApkError::Truncated)?;
        let out = match e.method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.usize_ as usize)
                .map_err(|_| ApkError::Corrupt(format!("bad deflate stream in {path}")))?,
            _ => return Err(ApkError::Unsupported("compression method")),
        };
        if out.len() != e.usize_ as usize || crc32(&out) != e.crc {
            return Err(ApkError::Corrupt(format!("size or CRC mismatch in {path}")));
        }
        Ok(out)
    }

    /// Read `lib/armeabi-v7a/<name>`.
    pub fn read_lib(&self, name: &str) -> Result<Vec<u8>, ApkError> {
        self.read(&format!("{LIB_DIR}{name}"))
    }

    /// Identify the build by the SHA-256 of `libApplication.so` against [`KNOWN_BUILDS`].
    ///
    /// Unknown builds are refused unless `force`, in which case `Ok(None)` is returned.
    pub fn verify(&self, force: bool) -> Result<Option<Release>, ApkError> {
        self.verify_with(&KNOWN_BUILDS, force)
    }

    /// [`Apk::verify`] against a caller-supplied table.
    pub fn verify_with(&self, table: &[(Release, &str)], force: bool) -> Result<Option<Release>, ApkError> {
        let hash = sha256_hex(&self.read_lib(IDENTITY_LIB)?);
        match table.iter().find(|(_, h)| *h == hash) {
            Some((r, _)) => Ok(Some(*r)),
            None if force => Ok(None),
            None => Err(ApkError::UnknownBuild { sha256: hash }),
        }
    }
}

impl LibSource for Apk {
    fn fetch(&mut self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        match self.read_lib(name) {
            Ok(b) => Ok(Some(b)),
            Err(ApkError::Missing(_)) => Ok(None),
            Err(e) => Err(LoadError::Source(e.to_string())),
        }
    }
}
