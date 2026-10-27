// SPDX-License-Identifier: GPL-3.0-or-later
//! Builds the vendored dynarmic (A32 frontend only) and the C ABI shim, then links the static libs.
//! A build whose inputs match an earlier one (any checkout, worktree or profile) reuses its archives from the shared
//! cache in `buildcache.rs` instead of running CMake again.
use std::path::{Path, PathBuf};

#[path = "src/buildcache.rs"]
mod buildcache;
#[path = "src/buildcfg.rs"]
mod buildcfg;

fn collect_archives(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_archives(&p, out);
        } else if buildcfg::is_static_lib(&p) {
            out.push(p);
        }
    }
}

/// `CARGO_CFG_TARGET_<name>` of the target being built (not the host running this script).
fn target_var(name: &str) -> String {
    std::env::var(format!("CARGO_CFG_TARGET_{name}")).unwrap_or_default()
}

fn main() {
    // `BHPC_SKIP_NATIVE=1`: type-check only (`cargo check --target <foreign triple>` without that target's C++
    // toolchain). Nothing is built or linked, so a link of the result fails.
    println!("cargo:rerun-if-env-changed=BHPC_SKIP_NATIVE");
    if std::env::var_os("BHPC_SKIP_NATIVE").is_some_and(|v| v == "1") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dynarmic = manifest.join("../../third_party/dynarmic").canonicalize().expect("third_party/dynarmic missing");
    // Windows canonicalizes to a verbatim `\\?\D:\...` path, which MSBuild cannot open sources from.
    let dynarmic = match dynarmic.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) if !plain.starts_with("UNC") => PathBuf::from(plain),
        _ => dynarmic,
    };
    let shim = manifest.join("shim");
    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed={}", manifest.join("cmake/CMakeLists.txt").display());
    println!("cargo:rerun-if-changed={}", dynarmic.join("src").display());

    let (os, arch, env) = (target_var("OS"), target_var("ARCH"), target_var("ENV"));
    let target = buildcfg::Target { os: &os, arch: &arch, env: &env };
    println!("cargo:rerun-if-env-changed=BHPC_BOOST_ROOT");
    let boost = std::env::var("BHPC_BOOST_ROOT").ok();

    let defines = buildcfg::cmake_defines(&target, boost.as_deref());

    let triple = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rerun-if-env-changed=BHPC_DYNARMIC_CACHE");
    println!("cargo:rerun-if-env-changed=XDG_CACHE_HOME");
    let entry = buildcache::cache_root(|k| std::env::var(k).ok()).map(|root| {
        let mut h = buildcache::Fnv::new();
        h.field(buildcache::SCHEMA.as_bytes());
        h.field(triple.as_bytes());
        for (k, v) in &defines {
            h.field(k.as_bytes());
            h.field(v.as_bytes());
        }
        for k in buildcache::env_inputs(&triple) {
            println!("cargo:rerun-if-env-changed={k}");
            h.field(k.as_bytes());
            h.field(std::env::var(&k).unwrap_or_default().as_bytes());
        }
        let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
        h.field(&buildcache::tool_version(&cxx));
        h.field(&buildcache::tool_version("cmake"));
        for dir in [&dynarmic, &shim, &manifest.join("cmake")] {
            buildcache::hash_tree(&mut h, dir).expect("cannot hash the dynarmic sources");
        }
        root.join(format!("dynarmic-{}", h.hex()))
    });

    let mut libs = match entry.as_deref().and_then(buildcache::lookup) {
        Some(cached) => {
            // Linked straight from the cache: rebuild if the entry is removed.
            println!("cargo:rerun-if-changed={}", entry.as_deref().unwrap().join(buildcache::COMPLETE).display());
            cached
        }
        None => {
            let libs = cmake_build(&dynarmic, &shim, &manifest, &defines);
            if let Some(e) = &entry {
                if let Err(err) = buildcache::store(e, &libs) {
                    println!("cargo:warning=dynarmic cache not written ({}): {err}", e.display());
                }
            }
            libs
        }
    };
    // Link order matters for static archives: shim first, then dynarmic, then its dependencies.
    libs.sort_by_key(|p| (buildcfg::rank(&p.file_name().unwrap().to_string_lossy()), p.clone()));
    for lib in &libs {
        let name = lib.file_stem().unwrap().to_string_lossy().to_string();
        let name = buildcfg::link_name(&name);
        println!("cargo:rustc-link-search=native={}", lib.parent().unwrap().display());
        println!("cargo:rustc-link-lib=static={name}");
    }
    if let Some(rt) = buildcfg::cxx_runtime(&target) {
        println!("cargo:rustc-link-lib=dylib={rt}");
    }
}

/// Runs CMake (dynarmic + shim, Release) and returns the static archives it produced.
fn cmake_build(dynarmic: &Path, shim: &Path, manifest: &Path, defines: &[(String, String)]) -> Vec<PathBuf> {
    let mut cfg = cmake::Config::new(manifest.join("cmake"));
    cfg.define("BHPC_DYNARMIC_DIR", dynarmic)
        .define("BHPC_SHIM_DIR", shim)
        // `profile` too, not only CMAKE_BUILD_TYPE: multi-config generators (Visual Studio) take the configuration
        // from `--config`, which the cmake crate otherwise derives from the cargo environment (a Debug dynarmic made
        // the game start four times slower on Windows).
        .profile("Release")
        .define("CMAKE_BUILD_TYPE", "Release")
        .build_target("bhpc_shim");
    for (k, v) in defines {
        cfg.define(k, v);
    }
    let build = cfg.build().join("build");
    let mut libs = Vec::new();
    collect_archives(&build, &mut libs);
    assert!(!libs.is_empty(), "no static libraries produced under {}", build.display());
    libs
}
