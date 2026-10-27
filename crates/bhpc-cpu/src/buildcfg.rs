// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-OS build decisions shared by `build.rs` (via `#[path]`) and the unit tests, so the logic that
//! only matters on Windows/macOS is still exercised on Linux.

use std::path::Path;

/// The parts of the Rust target triple that influence the native build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target<'a> {
    pub os: &'a str,
    pub arch: &'a str,
    pub env: &'a str,
}

impl Target<'_> {
    pub fn is_msvc(&self) -> bool {
        self.os == "windows" && self.env == "msvc"
    }
    pub fn is_apple(&self) -> bool {
        self.os == "macos"
    }
}

/// Is `path` a static library produced by CMake (`.a` for GNU toolchains, `.lib` for MSVC)?
pub fn is_static_lib(path: &Path) -> bool {
    path.extension().is_some_and(|x| x == "a" || x == "lib")
}

/// Link order bucket: shim first, then dynarmic, then its dependencies (static archives need this
/// on GNU ld; MSVC ignores it). Case-insensitive because MSVC names are `Zydis.lib`, GNU `libZydis.a`.
pub fn rank(file_name: &str) -> u8 {
    let n = file_name.to_ascii_lowercase();
    if n.contains("bhpc_shim") {
        0
    } else if n.contains("dynarmic") {
        1
    } else if n.contains("zydis") {
        2
    } else if n.contains("zycore") {
        3
    } else {
        4
    }
}

/// Name to pass to `rustc-link-lib` for an archive file stem (`libfoo` -> `foo`).
pub fn link_name(file_stem: &str) -> &str {
    file_stem.strip_prefix("lib").unwrap_or(file_stem)
}

/// The C++ runtime the final binary needs besides our static archives (`None` for MSVC, whose
/// runtime is selected by the compiler through default-lib directives).
pub fn cxx_runtime(t: &Target) -> Option<&'static str> {
    match (t.os, t.env) {
        ("windows", "msvc") => None,
        ("macos" | "ios" | "freebsd" | "openbsd", _) | ("android", _) => Some("c++"),
        _ => Some("stdc++"),
    }
}

/// `-D` definitions handed to CMake for this target, beyond the common ones in `CMakeLists.txt`.
pub fn cmake_defines(t: &Target, boost_root: Option<&str>) -> Vec<(String, String)> {
    let mut d = Vec::new();
    if t.is_msvc() {
        // Rust always links the dynamic CRT (/MD), even in debug builds; mixing /MT or /MDd breaks linking.
        d.push(("CMAKE_MSVC_RUNTIME_LIBRARY".into(), "MultiThreadedDLL".into()));
        // With the Visual Studio generator the cmake crate replaces the Release flags by its own, which carry no
        // optimisation: dynarmic was built with MSVC's default /Od and the game started four times slower. Give the
        // flags CMake's Release would (plus what the crate passes: /MD for the CRT, quiet, reproducible).
        for lang in ["C", "CXX"] {
            d.push((format!("CMAKE_{lang}_FLAGS_RELEASE"), "-nologo -MD -Brepro -W0 /O2 /Ob2 /DNDEBUG".into()));
        }
    }
    if t.is_apple() {
        let min = if t.arch == "aarch64" { "11.0" } else { "10.15" };
        d.push(("CMAKE_OSX_DEPLOYMENT_TARGET".into(), min.into()));
    }
    if let Some(root) = boost_root {
        d.push(("BOOST_ROOT".into(), root.into()));
        d.push(("Boost_INCLUDE_DIR".into(), format!("{root}/include")));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: Target = Target { os: "linux", arch: "x86_64", env: "gnu" };
    const WIN: Target = Target { os: "windows", arch: "x86_64", env: "msvc" };
    const WIN_GNU: Target = Target { os: "windows", arch: "x86_64", env: "gnu" };
    const MAC_ARM: Target = Target { os: "macos", arch: "aarch64", env: "" };
    const MAC_X64: Target = Target { os: "macos", arch: "x86_64", env: "" };

    #[test]
    fn archives_are_recognised() {
        assert!(is_static_lib(Path::new("a/libdynarmic.a")));
        assert!(is_static_lib(Path::new("a/Release/dynarmic.lib")));
        assert!(!is_static_lib(Path::new("a/dynarmic.so")));
        assert!(!is_static_lib(Path::new("a/noext")));
    }

    #[test]
    fn link_order_ranks() {
        let names = ["libfmt.a", "libZycore.a", "Zydis.lib", "libdynarmic.a", "bhpc_shim.lib", "libmcl.a"];
        let mut v: Vec<_> = names.iter().map(|n| rank(n)).collect();
        assert_eq!(v, [4, 3, 2, 1, 0, 4]);
        v.sort();
        assert_eq!(v, [0, 1, 2, 3, 4, 4]);
    }

    #[test]
    fn link_names() {
        assert_eq!(link_name("libfoo"), "foo");
        assert_eq!(link_name("Zydis"), "Zydis");
    }

    #[test]
    fn runtime_per_os() {
        assert_eq!(cxx_runtime(&LINUX), Some("stdc++"));
        assert_eq!(cxx_runtime(&WIN), None);
        assert_eq!(cxx_runtime(&WIN_GNU), Some("stdc++"));
        assert_eq!(cxx_runtime(&MAC_ARM), Some("c++"));
        assert_eq!(cxx_runtime(&Target { os: "android", arch: "aarch64", env: "" }), Some("c++"));
    }

    #[test]
    fn defines_per_target() {
        assert!(cmake_defines(&LINUX, None).is_empty());
        let w = cmake_defines(&WIN, Some("C:/boost"));
        assert!(w.contains(&("CMAKE_MSVC_RUNTIME_LIBRARY".into(), "MultiThreadedDLL".into())));
        assert!(w.iter().any(|(k, v)| k == "CMAKE_CXX_FLAGS_RELEASE" && v.contains("/O2") && v.contains("-MD")));
        assert!(w.iter().any(|(k, v)| k == "CMAKE_C_FLAGS_RELEASE" && v.contains("/O2")));
        assert!(w.contains(&("BOOST_ROOT".into(), "C:/boost".into())));
        assert!(w.contains(&("Boost_INCLUDE_DIR".into(), "C:/boost/include".into())));
        let a = cmake_defines(&MAC_ARM, None);
        assert_eq!(a, [("CMAKE_OSX_DEPLOYMENT_TARGET".to_string(), "11.0".to_string())]);
        assert_eq!(cmake_defines(&MAC_X64, None)[0].1, "10.15");
    }
}
