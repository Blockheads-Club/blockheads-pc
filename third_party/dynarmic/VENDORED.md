# Vendored dynarmic (A32 JIT)

- Upstream: <https://github.com/azahar-emu/dynarmic> (maintained fork of merryhime/dynarmic, which is gone)
- Commit: `a46601580d5512d324104f985b5f0209dc980ddc` (2026-09-26, "Delete redundant add_library alias")
- License: 0BSD (`LICENSE.txt`; SPDX headers verified in `src/dynarmic/interface/A32/*.h`)
- Copied verbatim (no local patches), minus: `.git*`, `tests/`, `docs/`, and `externals/{catch,biscuit}` plus test/doc/sample/example dirs of the externals.

## Externals (git submodule pins of the commit above, all vendored under `externals/`)

| Dir | Commit | License |
|---|---|---|
| fmt | 7b273fbb541c7c9058800bc613f4291274b2d26e | MIT |
| mcl (azahar-emu/mcl) | 5fc4beaf331037649b10625736b41365defb4f50 (0.1.14) | MIT |
| oaknut (arm64 hosts) | 94c726ce0338b054eb8cb5ea91de8fe6c19f4392 | MIT |
| robin-map | 054ec5ad67440fcd65e0497e5a27ef31f53fcc7f | MIT |
| xbyak (x86_64 hosts) | 8c0098b69f95d02dea47efcdd42ab5aa2d03c72d | BSD-3-Clause |
| zydis | bffbb610cfea643b98e87658b9058382f7522807 | MIT |
| zycore | 0b2432ced0884fd152b471d97ecf0258ff4d859f | MIT |

## System dependency

- Boost >= 1.57 headers (header-only use; Arch `boost`, Debian `libboost-dev`). `find_package(Boost)` is REQUIRED by dynarmic.
- CMake >= 3.12, a C++20 compiler (clang or gcc). No other system libraries (fmt/mcl/xbyak are the bundled copies).

Build flags used by `crates/bhpc-cpu/build.rs`: `DYNARMIC_FRONTENDS=A32`, `DYNARMIC_USE_BUNDLED_EXTERNALS=ON`, tests off, warnings-as-errors off.
