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

Build prerequisites (only x86_64 Linux is verified so far; arm64, Windows and macOS are planned for M8):

| Tool | Needed for | Notes |
|---|---|---|
| rustup, stable toolchain (>= 1.86) | everything | `rustup component add llvm-tools-preview` for coverage |
| CMake >= 3.16 | building dynarmic from `third_party/` | driven by `crates/bhpc-cpu/build.rs` |
| C++20 compiler (clang or gcc) | dynarmic and the shim in `crates/bhpc-cpu/shim` | |
| Boost headers | dynarmic (header-only use) | Arch `boost`, Debian/Ubuntu `libboost-dev`, Fedora `boost-devel` |
| `cargo-llvm-cov` | `scripts/coverage.sh` only | `cargo install cargo-llvm-cov` |

Every cargo profile (debug, release, the coverage target dir) builds its own copy of dynarmic: the first build of
each takes about 3.5 minutes, later builds are incremental.

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
scripts/coverage.sh            # fails below 100% line or function coverage
scripts/coverage.sh --html     # browsable report in target/llvm-cov/html
```

The gate (`scripts/coverage.sh`, wrap it in CI) requires **100% of lines and functions** of our own Rust code.
Only these are excluded: vendored `third_party/` (dynarmic), the C++ shim in `crates/bhpc-cpu/shim` (not Rust, so
never measured), and test code (`tests.rs`, `tests_*.rs`, `crates/*/tests/`, and the test-ELF builder
`crates/bhpc-loader/src/testelf.rs`). Set `COVERAGE_PACKAGES="bhpc-cpu bhpc-mem"` to gate a subset of crates.

## CPU embedding notes

- Thunks (host functions bound to `svc` slots) live in a `ThunkTable` shared by all `Cpu`s of a guest; use
  `Cpu::new_thread` for further guest threads (own JIT, shared memory, thunks and exclusive monitor).
- A thunk that needs a guest function's result (`qsort`/`bsearch` comparators, `atexit` handlers, dispatch
  callbacks) calls `Cpu::call` or `Cpu::call_or_stop` from inside the thunk. It runs on a child JIT on the same
  stack, nested up to `MAX_NEST_DEPTH` (16). Use `Cpu::tail_call` when the callee's result is the thunk's result.
- Undefined and unpredictable instructions stop the guest with `Stop::Undefined` / `Stop::Unpredictable`, which carry
  the pc and the instruction word.
