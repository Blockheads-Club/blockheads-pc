# blockheads-pc

Run **your own copy** of the Android build of The Blockheads natively on a PC. A Rust host loads the ARM32
(`armeabi-v7a`) shared objects from an APK **you supply**, executes them in an embedded ARM JIT
([dynarmic](third_party/dynarmic/VENDORED.md), 0BSD) and replaces Android with host-side re-implementations of
the system libraries (bionic, a fake JVM, GLES, audio, windowing). Licensed under GPL-3.0-only.

Status: **M0 (JIT spike)**. See [`docs/feasibility/PLAN.md`](docs/feasibility/PLAN.md) for the architecture and
milestones. Nothing here runs the game yet; the spike runs our own NDK-built test fixture.

## Legal notice

- You must supply your own, lawfully obtained APK. This project never downloads, bundles, extracts to disk, caches or
  redistributes game code or assets, and the repository contains none (see `.gitignore`).
- The only binary in the repository is `fixtures/hello.so`, built from our own C/C++ sources in `fixtures/`.
- Not affiliated with, endorsed by or sponsored by Majic Jungle or Noodlecake. "The Blockheads" belongs to its
  respective owners; the name is used only to describe what the software is compatible with.

## Layout

| Crate | Role |
|---|---|
| `bhpc-mem` | 4 GiB guest address window (host `mmap`), guest heap |
| `bhpc-cpu` | dynarmic A32 JIT behind a small C ABI shim (`crates/bhpc-cpu/shim`), SVC thunk ABI |
| `bhpc-loader` | ELF32/ARM loader: PT_LOAD mapping, RELATIVE/ABS32/GLOB_DAT/JUMP_SLOT, symbol binding |
| `bhpc-run` | M0 runner and thunk benchmark (`bhpc-run fixtures/hello.so`) |
| `bhpc-bionic`, `-jni`, `-gles`, `-audio`, `-app` | empty stubs for later milestones |

## Build and run

Requirements: rustup stable, CMake >= 3.16, a C++20 compiler (clang or gcc), Boost headers (header-only use by
dynarmic; Arch `boost`, Debian `libboost-dev`). The first build compiles dynarmic (about 4 minutes), later ones are
incremental.

```sh
cargo build --release
./target/release/bhpc-run fixtures/hello.so            # prints "hello from ARM" and the check results
./target/release/bhpc-run --bench-thunk                # thunk round-trip cost
./target/release/bhpc-run fixtures/hello.so --trace-thunks --watchdog 100000000 --trace-pc /tmp/pcs.txt
```

Rebuilding the fixture needs the Android NDK (r27, `ANDROID_NDK_HOME` or `~/Android/Sdk/ndk/27.2.12479018`):
`fixtures/build.sh`.

## Tests and coverage

```sh
cargo test --workspace
cargo llvm-cov --workspace --ignore-filename-regex '(/tests\.rs|testelf\.rs|third_party)' --summary-only
```

The C++ shim and the vendored dynarmic are not measured (third-party/glue); everything else is Rust and measured.
