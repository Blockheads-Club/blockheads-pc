//! Virtual file system of the Blockheads PC port (PLAN 1.8).
//!
//! * [`archive`]/[`asset`]/[`zipfs`]: the user's APK as a lazy, read-only tree. `AAsset`-style API
//!   for the JNI `AssetManager` ([`AssetManager`]) and a [`FileSystem`] view for bionic.
//! * [`hostfs`]/[`layout`]: the writable per-user data dir and the Android path mapping.
//! * [`memfs`]: synthetic `/proc`, `/sys`, `/dev`, `/etc`.
//! * [`fonts`]: the system font files and configuration CoreGraphics expects.
//! * [`vfs`]: the mount table tying it together; [`Storage`] builds the standard one.
//!
//! Nothing here executes guest code or touches the game on disk besides reading the APK.

pub mod archive;
pub mod asset;
pub mod error;
pub mod fonts;
pub mod fs;
pub mod hostfs;
pub mod layout;
pub mod memfs;
pub mod path;
pub mod vfs;
pub mod zipfs;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fonts;
#[cfg(test)]
mod tests_fs;
#[cfg(test)]
mod tests_layout;
#[cfg(test)]
mod tests_vfs;

pub use archive::ApkArchive;
pub use asset::{Asset, AssetDir, AssetManager, Whence};
pub use error::{Result, VfsError};
pub use fs::{DirEntry, FileKind, FileSystem, OpenFlags, Stat, VFile};
pub use layout::{build, DataDirs, GuestPaths, Storage, SystemInfo, VfsConfig};
pub use vfs::Vfs;
