//! "Import saves from Android": a folder or zip pulled from the game's data dir
//! (`/data/data/com.noodlecake.blockheads/...` or just its `files/`) mapped into the [`DataDirs`] layout.
//!
//! Mapping, after dropping everything up to and including the package directory when present:
//!
//! | Android | Host |
//! |---|---|
//! | `files/...` | `home/...` (the guest `HOME`) |
//! | `cache/...`, `code_cache/...` | skipped (regenerable) |
//! | `shared_prefs/...`, `databases/...`, `no_backup/...` | `appdata/<same>` |
//! | anything else | `home/...` (a bare `files/` pull is the common case) |

use bhpc_vfs::layout::DEFAULT_PACKAGE;
use bhpc_vfs::DataDirs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// What to do with a file that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Existing {
    #[default]
    Skip,
    Overwrite,
}

/// Import failures.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    Io(String),
    NotZip(String),
    /// The source is neither a directory nor a zip file.
    Source,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io(e) => write!(f, "i/o error: {e}"),
            ImportError::NotZip(e) => write!(f, "bad zip: {e}"),
            ImportError::Source => write!(f, "choose a folder or a .zip file"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<zip::result::ZipError> for ImportError {
    fn from(e: zip::result::ZipError) -> Self {
        ImportError::NotZip(e.to_string())
    }
}

impl From<io::Error> for ImportError {
    fn from(e: io::Error) -> Self {
        ImportError::Io(e.to_string())
    }
}

/// What happened.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Host files written (new or overwritten).
    pub written: Vec<PathBuf>,
    /// Files left alone because they already existed.
    pub skipped_existing: usize,
    /// Files not imported on purpose (caches, unsafe paths, links).
    pub ignored: usize,
}

/// One imported item for the results list: the first two path parts below the data root's `home/` or `appdata/`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportItem {
    pub name: String,
    pub files: usize,
    pub bytes: u64,
}

/// What the UI shows after an import.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportSummary {
    pub written: usize,
    pub skipped_existing: usize,
    pub ignored: usize,
    pub items: Vec<ImportItem>,
}

impl Report {
    /// Groups the written files into items (sorted by name) for display.
    pub fn summary(&self, dirs: &DataDirs) -> ImportSummary {
        let mut groups: std::collections::BTreeMap<String, (usize, u64)> = std::collections::BTreeMap::new();
        for p in &self.written {
            let rel = p.strip_prefix(&dirs.root).unwrap_or(p);
            let parts: Vec<String> =
                rel.components().skip(1).take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let g = groups.entry(parts.join("/")).or_default();
            g.0 += 1;
            g.1 += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        }
        ImportSummary {
            written: self.written.len(),
            skipped_existing: self.skipped_existing,
            ignored: self.ignored,
            items: groups.into_iter().map(|(name, (files, bytes))| ImportItem { name, files, bytes }).collect(),
        }
    }
}

/// Where the Android-relative `path` goes below `dirs`, `None` when it is not imported.
pub fn map_path(dirs: &DataDirs, path: &str) -> Option<PathBuf> {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split(['/', '\\']) {
        match c {
            "" | "." => {}
            ".." => return None,
            c if c.contains(':') => return None,
            c => parts.push(c),
        }
    }
    if let Some(i) = parts.iter().position(|p| *p == DEFAULT_PACKAGE) {
        parts.drain(..=i);
    }
    let (head, rest) = parts.split_first()?;
    let (base, tail): (&Path, &[&str]) = match *head {
        "cache" | "code_cache" => return None,
        "files" => (&dirs.home, rest),
        "shared_prefs" | "databases" | "no_backup" => (&dirs.appdata, parts.as_slice()),
        _ => (&dirs.home, parts.as_slice()),
    };
    if tail.is_empty() {
        return None;
    }
    Some(tail.iter().fold(base.to_path_buf(), |p, c| p.join(c)))
}

fn place(
    dirs: &DataDirs,
    rel: &str,
    data: &mut dyn Read,
    policy: Existing,
    report: &mut Report,
) -> Result<(), ImportError> {
    let Some(dest) = map_path(dirs, rel) else {
        report.ignored += 1;
        return Ok(());
    };
    if dest.exists() && policy == Existing::Skip {
        report.skipped_existing += 1;
        return Ok(());
    }
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new("")))?;
    io::copy(data, &mut std::fs::File::create(&dest)?)?;
    report.written.push(dest);
    Ok(())
}

fn walk(dirs: &DataDirs, root: &Path, rel: &str, policy: Existing, report: &mut Report) -> Result<(), ImportError> {
    let mut entries: Vec<_> = std::fs::read_dir(root.join(rel))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let ty = e.file_type()?;
        if ty.is_dir() {
            walk(dirs, root, &child, policy, report)?;
        } else if ty.is_file() {
            place(dirs, &child, &mut std::fs::File::open(e.path())?, policy, report)?;
        } else {
            report.ignored += 1; // symlinks and specials are never followed
        }
    }
    Ok(())
}

/// Imports `src` (a folder or a `.zip`) into `dirs`, creating the layout first.
pub fn import_saves(src: &Path, dirs: &DataDirs, policy: Existing) -> Result<Report, ImportError> {
    dirs.create_all()?;
    let mut report = Report::default();
    if src.is_dir() {
        walk(dirs, src, "", policy, &mut report)?;
    } else if src.is_file() {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(src)?)?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().to_string();
            place(dirs, &name, &mut f, policy, &mut report)?;
        }
    } else {
        return Err(ImportError::Source);
    }
    Ok(report)
}
