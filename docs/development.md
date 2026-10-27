# Developing Blockheads PC

Everything needed to build, run and test blockheads-pc from source. Architecture and milestones:
[feasibility/PLAN.md](feasibility/PLAN.md). Playing (controls, saves, known issues): [playing.md](playing.md).
The launcher (`app/`): [launcher.md](launcher.md).

## How it works

Run **your own copy** of the Android build of The Blockheads natively on a PC. A Rust host loads the ARM32
(`armeabi-v7a`) shared objects from an APK **you supply**, executes them in an embedded ARM JIT
([dynarmic](../third_party/dynarmic/VENDORED.md), 0BSD) and replaces Android with host-side re-implementations of
the system libraries (bionic, a fake JVM, GLES, audio, windowing). Licensed under GPL-3.0-or-later.

You must supply your own, lawfully obtained APK. The project never downloads, bundles, extracts to disk, caches or
redistributes game code or assets, and the repository contains none (see `.gitignore`). The only binary in the
repository is `fixtures/hello.so`, built from our own C/C++ sources in `fixtures/`.

## Layout

| Crate | Role |
|---|---|
| `bhpc-mem` | 4 GiB guest address window (host `mmap`), guest heap |
| `bhpc-cpu` | dynarmic A32 JIT behind a small C ABI shim (`crates/bhpc-cpu/shim`), SVC thunk ABI, cycle budget and time slices |
| `bhpc-loader` | ELF32/ARM loader: PT_LOAD mapping, relocations, symbol binding, constructors, EHABI backtraces, APK access |
| `bhpc-bionic` | host re-implementation of the bionic libc the game imports: stdio and `scanf`, string, math, time (host time zone), pthread and a guest scheduler (threads run in parallel, host functions behind one lock; `--serial-guest` for one at a time), futex, files |
| `bhpc-vfs` | Lazy read-only APK assets, writable data dir, Android path mapping, system fonts (OFL font bundled) |
| `bhpc-jni` | permissive fake `JavaVM`/`JNIEnv` |
| `bhpc-gles`, `bhpc-audio`, `bhpc-input`, `bhpc-app` | OpenGL ES / EGL, audio, input mapping, window and main loop (later milestones) |
| `bhpc-run` | runner: M0 fixture runner and benchmark, `--apk ... --init-only` (M1), and play mode with `--features play` |

## Build

Supported hosts: Linux, macOS and Windows, each on x86_64 and arm64 (built and tested in CI: `ci.yml` for Linux
x86_64 with the coverage gate, `ci-crossplat.yml` for the other five; `launcher-bundles.yml` makes the installers).
Build prerequisites:

| Tool | Needed for | Notes |
|---|---|---|
| rustup, stable toolchain (>= 1.86) | everything | `rustup component add llvm-tools-preview` for coverage |
| CMake >= 3.16 | building dynarmic from `third_party/` | driven by `crates/bhpc-cpu/build.rs` |
| C++20 compiler (clang or gcc) | dynarmic and the shim in `crates/bhpc-cpu/shim` | |
| Boost headers | dynarmic (header-only use) | Arch `boost`, Debian/Ubuntu `libboost-dev`, Fedora `boost-devel`, macOS `brew install boost`; elsewhere point `BHPC_BOOST_ROOT` at a tree with `include/boost` |
| Visual Studio 2022 Build Tools (C++, plus the ARM64 tools on arm64) | Windows only | MSVC builds dynarmic and the shim |
| `cargo-llvm-cov` | `scripts/coverage.sh` only | `cargo install cargo-llvm-cov` |

dynarmic and the shim are built once per machine: `crates/bhpc-cpu/build.rs` keeps the archives in
`~/.cache/bhpc/dynarmic-<hash>` (the hash covers the vendored sources, the shim, the CMake wrapper, the target, the
compiler and its flags) and every later build with the same inputs links them from there, whatever the profile, the
target dir or the checkout. Only the first build on a machine pays the **~1.5-3.5 minutes** of CMake.
`BHPC_DYNARMIC_CACHE=0` turns the cache off, `BHPC_DYNARMIC_CACHE=DIR` moves it. CI also caches the dynarmic build.

```sh
cargo build --release
./target/release/bhpc-run fixtures/hello.so            # prints "hello from ARM" and the check results
./target/release/bhpc-run --bench-thunk                # thunk round-trip cost
./target/release/bhpc-run fixtures/hello.so --trace-thunks --watchdog 100000000 --trace-pc /tmp/pcs.txt
```

## Running from source (Linux)

You supply the APK (a 1.7.4, 1.7.5 or 1.7.6 build); nothing is downloaded, extracted into the repository or cached
outside the data directory.

**Prerequisites:** the build tools from the table above, plus the SDL2 runtime and development files
(Arch `sdl2`, Debian/Ubuntu `libsdl2-dev`, Fedora `SDL2-devel`) and a working OpenGL (ES) capable display. Audio
goes through SDL2 as well.

**Build** (needs the `play` feature, which pulls in SDL2):

```sh
cargo build --release -p bhpc-run --features play
```

**Run with a real window:**

```sh
./target/release/bhpc-run --apk "$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk"
```

The first start takes a few seconds to load the libraries. The game's system alerts (the consent question "Enable
multiplayer and free crystals?", "Exit?", ...) are shown as dialogs: click a button, or use the arrow keys/Tab and
Enter (Escape picks the negative button; controller: D-pad, A, B). `--auto-alerts` answers them with their first
button instead (headless runs), `--alert-choice N` with button N. Closing the window lets the game save (it goes
to the background and terminates) before the run ends.

Useful flags (all optional):

| Flag | Effect |
|---|---|
| `--data-dir DIR` | where saves and settings live (see below) |
| `--window WxH` | window size, for example `--window 1280x960` |
| `--seconds N` | quit after N seconds |
| `--shot FILE.png` with `--shot-at N` | save a screenshot of swap N (and `FILE-first.png` of the first frame); `--exit-after-shot` quits afterwards |
| `--script FILE` | scripted input, see below; `--click X,Y@MS` is a one-off click at a time in ms; `--shot-dir DIR` is where script `shot` steps go |
| `--force` | run a build that is not one of the known versions |
| `--hide-dpad` | keep the game's on-screen D-pad hidden (a controller still walks with it) |
| `--pad-tile-cursor` | the controller cursor moves tile by tile in a world (right stick and D-pad), staying on its tile when the camera moves |
| `--serial-guest` | run the game's threads one at a time instead of in parallel on several cores (troubleshooting; `BHPC_PARALLEL_GUEST=0` does the same) |
| `--trace-objc`, `--trace-fs`, `--trace-jni`, `--trace-jni-calls`, `--trace-fn LIST`, `--jni-report FILE` | tracing of the guest's Objective-C messages, file access and the fake JVM; `BHPC_TRACE_NET=1` in the environment logs every socket call |
| `-v` | verbose |

At the end of a run the host prints the Java members that are still stubs and were called, the hottest host
functions and frame-time statistics.

For a run on a machine without a display (or to test without any sound) use `SDL_VIDEODRIVER=offscreen` (or
`xvfb-run`) and `SDL_AUDIODRIVER=dummy`; set `SDL_AUDIODRIVER=dummy` whenever you do not want to hear the game.

**Scripted input.** A script is one command per line, a time first (`1500` = ms since the first frame, `+300` = ms after
the previous command): `click X Y`, `drag X1 Y1 X2 Y2 MS`, `hold X Y MS`, `text ...`, `key NAME`, `keydown`/`keyup NAME`,
`shot NAME`, `waittext TEXT`, `clicktext TEXT`, `dumpviews`, `mark NAME`, `quit`. Coordinates are window pixels.
Full syntax in `crates/bhpc-run/src/script.rs`; a worked example (joining a server) is in
[`m7/net-bringup.md`](m7/net-bringup.md).

**Launcher.** `app/` is the "Blockheads PC" launcher (Tauri 2 + SvelteKit): a setup wizard (game file, window and
graphics, controls, import Android saves), a Home screen that starts and supervises the game, and Settings. It keeps
its choices in `~/.config/blockheads-pc/launcher.toml` and starts the `bhpc-run` sidecar with `--config` pointing at
that file. See [`launcher.md`](launcher.md) for building it, the installers and what is not wired yet
(fullscreen, vsync, key bindings and the game window title/icon are saved but not applied by `bhpc-run` yet).
`bhpc-run --config launcher.toml` also works on its own; flags next to it win over the file.

**Load only** (no window, runs every constructor and prints the thunk histogram; exit code 0 = fine, about 2 s):

```sh
./target/release/bhpc-run --apk "$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" --init-only --data-dir /tmp/bhpc-data
```

Further reading: [`m3/play-bringup.md`](m3/play-bringup.md) (play mode internals),
[`m7/net-bringup.md`](m7/net-bringup.md) (networking). The smoke test runs the load path against a real APK;
it is `#[ignore]`d and needs the APK:

```sh
BHPC_APK="$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" cargo test -p bhpc-run --test smoke -- --ignored --nocapture
```

Rebuilding the fixture needs the Android NDK (r27, `ANDROID_NDK_HOME` or `~/Android/Sdk/ndk/27.2.12479018`):
`fixtures/build.sh`.

## Tests and coverage

Two tiers; both keep the 100 % coverage rule:

```sh
scripts/check-fast.sh          # every commit: only the crates the change touches (~1 min)
scripts/check-full.sh          # before merging into dev: everything below plus a headless game smoke test
scripts/coverage.sh            # fails below 100% line or function coverage (whole workspace)
scripts/coverage.sh --html     # browsable report in target/llvm-cov/html
```

* `check-fast.sh` diffs against the merge base with `dev` (`BASE=<ref>` to change; working-tree changes count) and
  runs, for the changed crates: `cargo fmt --check`, `clippy -D warnings`, `scripts/coverage.sh` limited to them
  (`COVERAGE_PACKAGES`; this runs their tests and enforces 100 %) and their doctests; then `cargo test` for the
  workspace crates that depend on them, and the app's vitest (100 % thresholds) when `app/` changed. A change to
  `Cargo.toml`, `Cargo.lock`, `third_party/` or the coverage script selects every crate. `--dry-run` prints the
  selection; `CRATES="bhpc-x bhpc-y"` overrides it.
* `check-full.sh` runs `cargo fmt --check`, `clippy --workspace --all-targets --features bhpc-run/play -D warnings`,
  `scripts/coverage.sh` (every workspace test, instrumented, 100 % gate), the doctests, and in parallel the app's
  `bun run check`, vitest (100 %) and Playwright e2e on a free port (`PW_PORT`); then `scripts/smoke-game.sh`.
* `scripts/smoke-game.sh` builds the release play binary and runs the game once under Xvfb with a private D-Bus
  session, HOME and data dir: boot, the first-run world creation, in the world, exit to the main menu, under a time
  limit (`BHPC_SMOKE_TIMEOUT`, default 150 s; about 35 s here). It needs your own APK (`BHPC_APK`, else the
  launcher's imported copy, else `~/Downloads/The Blockheads_1.7.6.apk`) and is skipped without one.
* Both scripts use `sccache` as `RUSTC_WRAPPER` when it is installed (`cargo install sccache --locked
  --no-default-features`; `BHPC_SCCACHE=0` opts out): crates.io dependencies without build scripts are then compiled
  once per machine instead of once per checkout. Each checkout keeps its own `target/` (a shared `CARGO_TARGET_DIR`
  would make parallel agent worktrees wait on cargo's lock and rebuild each other's workspace crates).

The gate (`scripts/coverage.sh`, run by the GitHub Actions workflow `.github/workflows/ci.yml` together with fmt,
build, `clippy -D warnings` and the tests) requires **100% of lines and functions** of our own Rust code.
Only these are excluded: vendored `third_party/` (dynarmic), the C++ shim in `crates/bhpc-cpu/shim` (not Rust, so
never measured), and test code (`tests.rs`, `tests_*.rs`, `crates/*/tests/`, and the test-ELF builder
`crates/bhpc-loader/src/testelf.rs`). Set `COVERAGE_PACKAGES="bhpc-cpu bhpc-mem"` to gate a subset of crates.

## Releases

Releases are automatic (semantic-release, `.releaserc.json`). When `ci` passes for a push to `dev`,
`.github/workflows/auto-release.yml` reads the Conventional Commits since the last `v*` tag: `fix`/`perf` cut a patch
release, `feat` a minor one, `BREAKING CHANGE` a major one; other types cut none. It sets the version
(`scripts/set-version.mjs`), commits `chore(release): vX.Y.Z`, tags it, opens a draft release with the generated
notes and dispatches `launcher-bundles.yml` with `release_tag` and `publish`, which builds that tag, attaches every
installer and `latest.json`, publishes the release and rebuilds the website. Edit the notes on GitHub afterwards if
they need more than the commit subjects.

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
