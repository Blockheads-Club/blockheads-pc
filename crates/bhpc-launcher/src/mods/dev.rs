//! Developer mode: mods loaded straight from a source folder (the `bhmod check` layout). The launcher checks the
//! folder and remembers it and passes the folders to the game (`--mods-dev`); [`reload`] asks the running game to load
//! them again, [`fingerprint`] tells when a folder's files changed (live reload).

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

/// Asks the running game (its stdin, `None` when no game runs in the launcher's window) to reload its mods.
pub fn reload(input: Option<&std::sync::mpsc::Sender<String>>) -> MRes<()> {
    let tx = input.ok_or_else(|| ModError::new("notRunning"))?;
    tx.send(crate::embed::Cmd::ModsReload.encode()).map_err(|_| ModError::new("notRunning"))
}

/// A number that changes when a file below `path` is added, removed, resized or modified.
pub fn fingerprint(path: &Path) -> u64 {
    fn walk(p: &Path, acc: &mut u64) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(md) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let mtime =
                md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
            for b in name.bytes().chain(md.len().to_le_bytes()).chain(mtime.as_nanos().to_le_bytes()) {
                *acc = (*acc ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
            if md.is_dir() {
                walk(&e.path(), acc);
            }
        }
    }
    let mut acc = 0xcbf2_9ce4_8422_2325;
    walk(path, &mut acc);
    acc
}
