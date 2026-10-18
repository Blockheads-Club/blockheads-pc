//! First-run APK validation.

use bhpc_loader::apk::{Apk, ApkError, Release, KNOWN_BUILDS};
use serde::Serialize;
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

/// Outcome class of [`check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApkStatus {
    /// A known build (1.7.4, 1.7.5 or 1.7.6).
    Supported,
    /// Accepted only because unknown builds are allowed.
    Unknown,
    /// Not usable; see [`ApkReport::problem`].
    Error,
}

/// Why an APK was refused (the four error cards of the setup screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApkProblem {
    /// Not an APK, or an APK of another app.
    NotBlockheads,
    /// A Blockheads build we do not know.
    Unsupported,
    /// The archive is cut off or inconsistent.
    Damaged,
    /// The file cannot be opened (missing, permission).
    Unreadable,
}

/// Serialisable result of checking an APK; this is what the UI shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApkReport {
    pub path: String,
    pub size_bytes: u64,
    pub status: ApkStatus,
    /// `1.7.6` for a supported build.
    pub version: Option<String>,
    pub problem: Option<ApkProblem>,
    /// The technical message (for "Show details").
    pub detail: String,
}

/// Maps a reader failure onto the error card to show.
pub fn problem_of(e: &ApkError) -> ApkProblem {
    match e {
        ApkError::Io(_) => ApkProblem::Unreadable,
        ApkError::NotZip | ApkError::Missing(_) => ApkProblem::NotBlockheads,
        ApkError::Truncated | ApkError::Corrupt(_) | ApkError::Unsupported(_) => ApkProblem::Damaged,
        ApkError::UnknownBuild { .. } => ApkProblem::Unsupported,
    }
}

/// Checks `path` and describes the result; never fails.
pub fn check(path: &Path, force: bool) -> ApkReport {
    check_with(path, &KNOWN_BUILDS, force)
}

/// [`check`] against a caller-supplied hash table.
pub fn check_with(path: &Path, table: &[(Release, &str)], force: bool) -> ApkReport {
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut r = ApkReport {
        path: path.display().to_string(),
        size_bytes,
        status: ApkStatus::Error,
        version: None,
        problem: None,
        detail: String::new(),
    };
    match validate_with(path, table, force) {
        Ok(ApkInfo { release: Some(rel), .. }) => {
            r.status = ApkStatus::Supported;
            r.version = Some(rel.version().to_string());
        }
        Ok(ApkInfo { release: None, .. }) => r.status = ApkStatus::Unknown,
        Err(e) => {
            r.problem = Some(problem_of(&e));
            r.detail = e.to_string();
        }
    }
    r
}
