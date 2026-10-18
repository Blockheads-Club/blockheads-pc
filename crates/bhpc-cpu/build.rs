// SPDX-License-Identifier: GPL-3.0-only
//! Builds the vendored dynarmic (A32 frontend only) and the C ABI shim, then links the static libs.
use std::path::{Path, PathBuf};

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
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dynarmic = manifest.join("../../third_party/dynarmic").canonicalize().expect("third_party/dynarmic missing");
    let shim = manifest.join("shim");
    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed={}", manifest.join("cmake/CMakeLists.txt").display());
    println!("cargo:rerun-if-changed={}", dynarmic.join("src").display());

    let (os, arch, env) = (target_var("OS"), target_var("ARCH"), target_var("ENV"));
    let target = buildcfg::Target { os: &os, arch: &arch, env: &env };
    println!("cargo:rerun-if-env-changed=BHPC_BOOST_ROOT");
    let boost = std::env::var("BHPC_BOOST_ROOT").ok();

    let mut cfg = cmake::Config::new(manifest.join("cmake"));
    cfg.define("BHPC_DYNARMIC_DIR", &dynarmic)
        .define("BHPC_SHIM_DIR", &shim)
        .define("CMAKE_BUILD_TYPE", "Release")
        .build_target("bhpc_shim");
    for (k, v) in buildcfg::cmake_defines(&target, boost.as_deref()) {
        cfg.define(k, v);
    }
    let dst = cfg.build();

    let build = dst.join("build");
    let mut libs = Vec::new();
    collect_archives(&build, &mut libs);
    assert!(!libs.is_empty(), "no static libraries produced under {}", build.display());
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
