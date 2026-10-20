// SPDX-License-Identifier: GPL-3.0-or-later
//! Prebuilt dynarmic cache shared by every checkout, worktree and target directory (`build.rs` includes this file
//! via `#[path]`; the unit tests below cover it on every OS).
//!
//! dynarmic and the shim are always built in CMake's Release mode with flags that do not depend on the cargo profile,
//! so one build serves `target/debug`, `target/release`, `cargo llvm-cov`'s target dir and every agent worktree. The
//! key hashes everything that goes into that build: the vendored sources, the shim, the CMake wrapper, the target,
//! the CMake defines, the compiler/CMake versions and the compiler environment variables. Entries live in
//! `<root>/dynarmic-<key>` (see [`cache_root`]); a missing or partial entry just means a normal CMake build.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::buildcfg::is_static_lib;

/// Bumped when the layout of an entry or the meaning of the key changes.
pub const SCHEMA: &str = "bhpc-dynarmic-cache-v1";

/// Written last into an entry: only entries that have it are used.
pub const COMPLETE: &str = ".complete";

/// 64-bit FNV-1a: deterministic across runs and Rust versions (std's `DefaultHasher` is not guaranteed to be).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// Hashes `b` with a length prefix, so `("ab", "c")` and `("a", "bc")` differ.
    pub fn field(&mut self, b: &[u8]) {
        self.bytes(&(b.len() as u64).to_le_bytes());
        self.bytes(b);
    }

    pub fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Feeds every file under `root` into `h`: sorted relative paths (with `/` separators) and their contents.
pub fn hash_tree(h: &mut Fnv, root: &Path) -> io::Result<()> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    for rel in files {
        h.field(rel.as_bytes());
        h.field(&std::fs::read(root.join(&rel))?);
    }
    Ok(())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect_files(root, &p, out)?;
        } else {
            out.push(p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Environment variables that change what the C/C++ compiler produces for `target` (the ones the `cc`/`cmake` crates
/// read, plain and target-specific).
pub fn env_inputs(target: &str) -> Vec<String> {
    let mut v: Vec<String> =
        ["CMAKE_GENERATOR", "CMAKE_TOOLCHAIN_FILE", "CMAKE_POLICY_VERSION_MINIMUM", "BHPC_BOOST_ROOT"]
            .map(String::from)
            .to_vec();
    for base in ["CC", "CXX", "CFLAGS", "CXXFLAGS", "AR"] {
        v.push(base.to_string());
        v.push(format!("TARGET_{base}"));
        v.push(format!("{base}_{target}"));
        v.push(format!("{base}_{}", target.replace('-', "_")));
    }
    v
}

/// Standard output of `<program> --version`, or nothing when the program cannot be run (MSVC has no `c++`).
pub fn tool_version(program: &str) -> Vec<u8> {
    Command::new(program).arg("--version").output().map(|o| o.stdout).unwrap_or_default()
}

/// The directory holding the cache entries, from the environment (`var`):
/// `BHPC_DYNARMIC_CACHE` (empty or `0` turns the cache off), else `$XDG_CACHE_HOME/bhpc`, `$HOME/.cache/bhpc` or
/// `%LOCALAPPDATA%\bhpc`.
pub fn cache_root(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty());
    if let Some(v) = var("BHPC_DYNARMIC_CACHE") {
        return (!v.is_empty() && v != "0").then(|| PathBuf::from(v));
    }
    if let Some(x) = set("XDG_CACHE_HOME") {
        return Some(Path::new(&x).join("bhpc"));
    }
    if let Some(h) = set("HOME") {
        return Some(Path::new(&h).join(".cache").join("bhpc"));
    }
    set("LOCALAPPDATA").map(|l| Path::new(&l).join("bhpc"))
}

/// The static archives of a complete entry, sorted, or `None` when the entry is missing, partial or empty.
pub fn lookup(entry: &Path) -> Option<Vec<PathBuf>> {
    if !entry.join(COMPLETE).is_file() {
        return None;
    }
    let mut libs: Vec<PathBuf> =
        std::fs::read_dir(entry).ok()?.flatten().map(|e| e.path()).filter(|p| is_static_lib(p)).collect();
    libs.sort();
    (!libs.is_empty()).then_some(libs)
}

/// Copies `libs` into a new entry: a private temporary directory renamed into place at the end, so readers never see
/// a partial entry. When another build created the entry first, ours is dropped.
pub fn store(entry: &Path, libs: &[PathBuf]) -> io::Result<()> {
    let parent = entry.parent().ok_or_else(|| io::Error::other("cache entry has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let name = entry.file_name().unwrap_or_default().to_string_lossy();
    let tmp = parent.join(format!(".tmp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for lib in libs {
        let file = lib.file_name().ok_or_else(|| io::Error::other("archive without a file name"))?;
        std::fs::copy(lib, tmp.join(file))?;
    }
    std::fs::write(tmp.join(COMPLETE), SCHEMA)?;
    if std::fs::rename(&tmp, entry).is_err() {
        std::fs::remove_dir_all(&tmp)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bhpc-buildcache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fnv_is_stable_and_length_prefixed() {
        let mut a = Fnv::new();
        a.field(b"ab");
        a.field(b"c");
        let mut b = Fnv::new();
        b.field(b"a");
        b.field(b"bc");
        assert_ne!(a, b);
        let mut e = Fnv::new();
        e.field(b"");
        assert_eq!(e.hex(), "a8c7f832281a39c5");
        assert_eq!(Fnv::new().hex().len(), 16);
    }

    #[test]
    fn tree_hash_follows_paths_and_contents() {
        let d = scratch("tree");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "one").unwrap();
        std::fs::write(d.join("sub/b.txt"), "two").unwrap();
        let hash = || {
            let mut h = Fnv::new();
            hash_tree(&mut h, &d).unwrap();
            h
        };
        let first = hash();
        assert_eq!(hash(), first);
        std::fs::write(d.join("sub/b.txt"), "TWO").unwrap();
        let changed = hash();
        assert_ne!(changed, first);
        std::fs::rename(d.join("sub/b.txt"), d.join("sub/c.txt")).unwrap();
        assert_ne!(hash(), changed);
        assert!(hash_tree(&mut Fnv::new(), &d.join("missing")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn env_inputs_cover_target_specific_names() {
        let v = env_inputs("x86_64-unknown-linux-gnu");
        for k in
            ["CXX", "TARGET_CXX", "CXX_x86_64-unknown-linux-gnu", "CFLAGS_x86_64_unknown_linux_gnu", "CMAKE_GENERATOR"]
        {
            assert!(v.iter().any(|x| x == k), "{k}");
        }
    }

    #[test]
    fn tool_versions() {
        assert!(!tool_version("cargo").is_empty());
        assert!(tool_version("bhpc-no-such-program").is_empty());
    }

    #[test]
    fn cache_root_from_env() {
        let root = |pairs: &[(&str, &str)]| {
            let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            cache_root(|k| m.get(k).cloned())
        };
        assert_eq!(root(&[("BHPC_DYNARMIC_CACHE", "/c"), ("HOME", "/h")]), Some(PathBuf::from("/c")));
        assert_eq!(root(&[("BHPC_DYNARMIC_CACHE", "0"), ("HOME", "/h")]), None);
        assert_eq!(root(&[("BHPC_DYNARMIC_CACHE", ""), ("HOME", "/h")]), None);
        assert_eq!(root(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")]), Some(PathBuf::from("/x/bhpc")));
        assert_eq!(root(&[("XDG_CACHE_HOME", ""), ("HOME", "/h")]), Some(Path::new("/h").join(".cache").join("bhpc")));
        assert_eq!(root(&[("LOCALAPPDATA", "C:/L")]), Some(Path::new("C:/L").join("bhpc")));
        assert_eq!(root(&[]), None);
    }

    #[test]
    fn store_then_lookup() {
        let d = scratch("store");
        let src = d.join("build");
        std::fs::create_dir_all(src.join("deep")).unwrap();
        std::fs::write(src.join("libdynarmic.a"), "dyn").unwrap();
        std::fs::write(src.join("deep/libbhpc_shim.a"), "shim").unwrap();
        let libs = [src.join("libdynarmic.a"), src.join("deep/libbhpc_shim.a")];
        let entry = d.join("cache/dynarmic-0123");

        assert_eq!(lookup(&entry), None);
        store(&entry, &libs).unwrap();
        let got = lookup(&entry).unwrap();
        assert_eq!(got, [entry.join("libbhpc_shim.a"), entry.join("libdynarmic.a")]);
        assert_eq!(std::fs::read_to_string(&got[1]).unwrap(), "dyn");

        // A second build that lost the race keeps the first entry and leaves no temporary directory behind.
        std::fs::write(src.join("libdynarmic.a"), "other").unwrap();
        store(&entry, &libs).unwrap();
        assert_eq!(std::fs::read_to_string(entry.join("libdynarmic.a")).unwrap(), "dyn");
        assert_eq!(std::fs::read_dir(d.join("cache")).unwrap().count(), 1);

        // Partial (no marker) and empty entries are ignored.
        std::fs::remove_file(entry.join(COMPLETE)).unwrap();
        assert_eq!(lookup(&entry), None);
        let empty = d.join("cache/dynarmic-empty");
        store(&empty, &[]).unwrap();
        assert_eq!(lookup(&empty), None);

        // Errors: no parent, a source that is not a file name, a missing source.
        assert!(store(Path::new("/"), &libs).is_err());
        assert!(store(&d.join("cache/x"), &[PathBuf::from("/")]).is_err());
        assert!(store(&d.join("cache/y"), &[src.join("missing.a")]).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
