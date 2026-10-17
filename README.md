# blockheads-pc

Run **your own copy** of the Android build of The Blockheads natively on a PC. A Rust host loads the ARM32
(`armeabi-v7a`) shared objects from an APK **you supply**, executes them in an embedded ARM JIT
([dynarmic](third_party/dynarmic/VENDORED.md), 0BSD) and replaces Android with host-side re-implementations of
the system libraries (bionic, a fake JVM, GLES, audio, windowing). Licensed under GPL-3.0-only.

Status: **M1 done** (G1 gate passed with fixes, see [`docs/m1/G1-decision.md`](docs/m1/G1-decision.md)); M2 (bionic
core and threads) is in progress. With your APK, all 25 ARM libraries load, about 262,000 relocations are applied and
every constructor (237 of 237) runs to completion on 1.7.4, 1.7.5 and 1.7.6. The game does not render or run its main
loop yet. See [`docs/feasibility/PLAN.md`](docs/feasibility/PLAN.md) for the architecture and milestones.

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
| `bhpc-cpu` | dynarmic A32 JIT behind a small C ABI shim (`crates/bhpc-cpu/shim`), SVC thunk ABI, cycle budget and time slices |
| `bhpc-loader` | ELF32/ARM loader: PT_LOAD mapping, relocations, symbol binding, constructors, EHABI backtraces, APK access |
| `bhpc-bionic` | host re-implementation of the bionic libc the game imports: stdio and `scanf`, string, math, time (host time zone), pthread and a cooperative guest scheduler with time slices, futex, files |
| `bhpc-vfs` | Lazy read-only APK assets, writable data dir, Android path mapping, system fonts (OFL font bundled) |
| `bhpc-jni` | permissive fake `JavaVM`/`JNIEnv` |
| `bhpc-gles`, `bhpc-audio`, `bhpc-input`, `bhpc-app` | OpenGL ES / EGL, audio, input mapping, window and main loop (later milestones) |
| `bhpc-run` | runner: M0 fixture runner and benchmark, and `--apk ... --init-only` (M1) |

## Build and run

Build prerequisites (only x86_64 Linux is verified so far; arm64, Windows and macOS are planned for M8):

| Tool | Needed for | Notes |
|---|---|---|
| rustup, stable toolchain (>= 1.86) | everything | `rustup component add llvm-tools-preview` for coverage |
| CMake >= 3.16 | building dynarmic from `third_party/` | driven by `crates/bhpc-cpu/build.rs` |
| C++20 compiler (clang or gcc) | dynarmic and the shim in `crates/bhpc-cpu/shim` | |
| Boost headers | dynarmic (header-only use) | Arch `boost`, Debian/Ubuntu `libboost-dev`, Fedora `boost-devel` |
| `cargo-llvm-cov` | `scripts/coverage.sh` only | `cargo install cargo-llvm-cov` |

Every cargo profile (debug, release, the coverage target dir) builds its own copy of dynarmic: the **first build of
each takes about 3.5 minutes** (a clean `cargo test --workspace` plus the coverage build is therefore about 8 to 10
minutes), later builds are incremental. CI caches the dynarmic build.

```sh
cargo build --release
./target/release/bhpc-run fixtures/hello.so            # prints "hello from ARM" and the check results
./target/release/bhpc-run --bench-thunk                # thunk round-trip cost
./target/release/bhpc-run fixtures/hello.so --trace-thunks --watchdog 100000000 --trace-pc /tmp/pcs.txt
```

### Running your APK

You supply the APK; nothing is downloaded, extracted into the repository or cached outside the data directory.

```sh
# Load the libraries, apply relocations, run every constructor, print the thunk histogram. Exit code 0 = all fine.
./target/release/bhpc-run --apk "$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" --init-only --data-dir /tmp/bhpc-data
# Useful switches: -v, --trace-thunks, --trace-fs, --trace-jni, --trace-pc FILE, --watchdog CYCLES, --keep-going, --top N
```

`--init-only` is the only mode implemented so far (about 2 s in a release build). The smoke test runs the same path
against a real APK and checks the ObjC runtime, CoreFoundation and `NSLog`; it is `#[ignore]`d and needs the APK:

```sh
BHPC_APK="$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" cargo test -p bhpc-run --test smoke -- --ignored --nocapture
```

Rebuilding the fixture needs the Android NDK (r27, `ANDROID_NDK_HOME` or `~/Android/Sdk/ndk/27.2.12479018`):
`fixtures/build.sh`.

## Tests and coverage

```sh
cargo test --workspace
scripts/coverage.sh            # fails below 100% line or function coverage
scripts/coverage.sh --html     # browsable report in target/llvm-cov/html
```

The gate (`scripts/coverage.sh`, run by the GitHub Actions workflow `.github/workflows/ci.yml` together with fmt,
build, `clippy -D warnings` and the tests) requires **100% of lines and functions** of our own Rust code.
Only these are excluded: vendored `third_party/` (dynarmic), the C++ shim in `crates/bhpc-cpu/shim` (not Rust, so
never measured), and test code (`tests.rs`, `tests_*.rs`, `crates/*/tests/`, and the test-ELF builder
`crates/bhpc-loader/src/testelf.rs`). Set `COVERAGE_PACKAGES="bhpc-cpu bhpc-mem"` to gate a subset of crates.

## CPU embedding notes

- Thunks (host functions bound to `svc` slots) live in a `ThunkTable` shared by all `Cpu`s of a guest; use
  `Cpu::new_thread` for further guest threads (own JIT, shared memory, thunks and exclusive monitor).
- A thunk that needs a guest function's result (`qsort`/`bsearch` comparators, `atexit` handlers, dispatch
  callbacks) calls `Cpu::call` or `Cpu::call_or_stop` from inside the thunk. It runs on a child JIT on the same
  stack, nested up to `MAX_NEST_DEPTH` (16). Use `Cpu::tail_call` when the callee's result is the thunk's result.
- `Cpu::set_slice(cycles, hook)` runs the guest in time slices (needs `cycle_counting` and a budget from
  `set_ticks`): when a slice is used up the hook runs and the guest continues. `bhpc_bionic::sched::enable_time_slice`
  installs the hook that yields the scheduler baton to the other guest threads (so spin-waits cannot starve their
  producer) and enforces the wall-clock limit set with `sched::set_wall_deadline`. Threads and nested calls inherit it.
- Undefined and unpredictable instructions stop the guest with `Stop::Undefined` / `Stop::Unpredictable`, which carry
  the pc and the instruction word.
