//! Developer mode: mods loaded straight from a source folder (the `bhmod check` layout). The launcher checks the
//! folder and remembers it; loading it into the game, reloading on change and the live log need the in-game loader
//! (milestone M6), so [`reload`] answers `needsLoader` until then.

use std::path::Path;

use bhmod_manifest::package::{self, CheckOptions, DirSource};
use serde::Serialize;

use super::{required_permissions, MRes, ModError};

/// A folder mod as the Developer tab shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DevMod {
    pub path: String,
    /// From `mod.toml` when it parses.
    pub id: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub permissions: Vec<String>,
    pub native: bool,
    /// Package problems (`path: message`); empty when the folder would pack cleanly.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Checks a mod source folder with the package rules.
pub fn inspect(path: &Path) -> MRes<DevMod> {
    let mut src = DirSource::open(path)
        .map_err(|e| ModError { file: Some(path.to_string_lossy().into()), ..ModError::with("io", e.to_string()) })?;
    let r = package::check(&mut src, CheckOptions::default());
    let m = r.manifest.as_ref();
    Ok(DevMod {
        path: path.to_string_lossy().into_owned(),
        id: m.map(|m| m.id.clone()),
        version: m.map(|m| m.version.clone()),
        name: m.and_then(|m| m.name.get("en").cloned()),
        permissions: m.map(|m| required_permissions(&m.permissions)).unwrap_or_default(),
        native: m.is_some_and(|m| m.permissions.native),
        errors: r.errors.iter().map(|i| i.to_string()).collect(),
        warnings: r.warnings.iter().map(|i| i.to_string()).collect(),
    })
}

/// Adds `path` to the developer folders (no duplicates), returning the new list.
pub fn add_folder(folders: &[String], path: &str) -> Vec<String> {
    let mut out = folders.to_vec();
    if !out.iter().any(|f| f == path) {
        out.push(path.to_string());
    }
    out
}

/// Loads or reloads a folder mod in the running game: needs the in-game mod loader (M6).
pub fn reload(_path: &Path) -> MRes<()> {
    Err(ModError::new("needsLoader"))
}
