//! Guest path normalisation.

use crate::error::{Result, VfsError};

/// Normalises a guest path to an absolute `/a/b` form.
///
/// Relative paths are resolved against `cwd` (which must itself be absolute). Empty components
/// and `.` vanish, `..` pops (and stops at the root, so a guest can never climb out of a mount).
/// An empty path is `ENOENT`, as on POSIX; a NUL byte is `EINVAL`.
pub fn normalize(cwd: &str, path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(VfsError::NotFound);
    }
    if path.contains('\0') {
        return Err(VfsError::Invalid);
    }
    let mut parts: Vec<&str> = Vec::new();
    let joined = if path.starts_with('/') { None } else { Some(cwd) };
    for comp in joined.into_iter().flat_map(|c| c.split('/')).chain(path.split('/')) {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

/// Splits a normalised path into its components (`"/"` has none).
pub fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|c| !c.is_empty())
}

/// `Some(rest)` when `path` equals `prefix` (rest is `"/"`) or lies below it (rest starts with `/`).
/// Both must be normalised; `prefix == "/"` matches everything.
pub fn strip_mount<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix == "/" {
        return Some(path);
    }
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() {
        Some("/")
    } else if rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}
