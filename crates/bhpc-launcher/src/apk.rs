//! First-run APK validation.

use bhpc_loader::apk::{Apk, ApkError, Release, KNOWN_BUILDS};
use std::path::{Path, PathBuf};

/// A validated APK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkInfo {
    /// Where the APK lives.
    pub path: PathBuf,
    /// The recognised release, `None` when accepted with `force`.
    pub release: Option<Release>,
}

impl ApkInfo {
    /// Human readable summary for the UI.
    pub fn describe(&self) -> String {
        match self.release {
            Some(r) => format!("Blockheads {} (supported)", r.version()),
            None => "unknown build (forced, may not work)".to_string(),
        }
    }
}

/// Checks that `path` is an APK holding a supported build (see [`KNOWN_BUILDS`]).
pub fn validate(path: &Path, force: bool) -> Result<ApkInfo, ApkError> {
    validate_with(path, &KNOWN_BUILDS, force)
}

/// [`validate`] against a caller-supplied hash table.
pub fn validate_with(path: &Path, table: &[(Release, &str)], force: bool) -> Result<ApkInfo, ApkError> {
    let release = Apk::open(path)?.verify_with(table, force)?;
    Ok(ApkInfo { path: path.to_path_buf(), release })
}
