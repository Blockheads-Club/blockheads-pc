// SPDX-License-Identifier: GPL-3.0-only
//! Builds the vendored dynarmic (A32 frontend only) and the C ABI shim, then links the static libs.
use std::path::{Path, PathBuf};

fn collect_archives(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_archives(&p, out);
        } else if p.extension().is_some_and(|x| x == "a") {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dynarmic = manifest.join("../../third_party/dynarmic").canonicalize().expect("third_party/dynarmic missing");
    let shim = manifest.join("shim");
    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed={}", manifest.join("cmake/CMakeLists.txt").display());
    println!("cargo:rerun-if-changed={}", dynarmic.join("src").display());

    let dst = cmake::Config::new(manifest.join("cmake"))
        .define("BHPC_DYNARMIC_DIR", &dynarmic)
        .define("BHPC_SHIM_DIR", &shim)
        .define("CMAKE_BUILD_TYPE", "Release")
        .build_target("bhpc_shim")
        .build();

    let build = dst.join("build");
    let mut libs = Vec::new();
    collect_archives(&build, &mut libs);
    assert!(!libs.is_empty(), "no static libraries produced under {}", build.display());
    // Link order matters for static archives: shim first, then dynarmic, then its dependencies.
    let rank = |p: &PathBuf| {
        let n = p.file_name().unwrap().to_string_lossy().to_string();
        if n.contains("bhpc_shim") {
            0
        } else if n.contains("dynarmic") {
            1
        } else if n.contains("Zydis") {
            2
        } else if n.contains("Zycore") {
            3
        } else {
            4
        }
    };
    libs.sort_by_key(|p| (rank(p), p.clone()));
    for lib in &libs {
        let name = lib.file_stem().unwrap().to_string_lossy().to_string();
        let name = name.strip_prefix("lib").unwrap_or(&name).to_string();
        println!("cargo:rustc-link-search=native={}", lib.parent().unwrap().display());
        println!("cargo:rustc-link-lib=static={name}");
    }
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
