//! Data folder helpers: size for display, copy for "Move…", and the private APK copy.

use std::io;
use std::path::{Path, PathBuf};

/// Total size in bytes of all regular files below `dir` (symlinks are not followed); 0 when it does not exist.
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Copies the tree `src` into `dst` (created), returning the number of files copied. Symlinks are skipped.
pub fn copy_dir(src: &Path, dst: &Path) -> io::Result<usize> {
    std::fs::create_dir_all(dst)?;
    let mut n = 0;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        let ty = e.file_type()?;
        if ty.is_dir() {
            n += copy_dir(&e.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(e.path(), &to)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copies the APK to `<data dir>/game/TheBlockheads.apk` and returns the new path.
pub fn keep_private_copy(apk: &Path, data_dir: &Path) -> io::Result<PathBuf> {
    let dest = data_dir.join("game").join("TheBlockheads.apk");
    if apk == dest {
        return Ok(dest);
    }
    std::fs::create_dir_all(data_dir.join("game"))?;
    std::fs::copy(apk, &dest)?;
    Ok(dest)
}
