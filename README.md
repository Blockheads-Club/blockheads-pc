# blockheads-pc

Run **your own copy** of the Android build of The Blockheads natively on a PC. A Rust host loads the ARM32
(`armeabi-v7a`) shared objects from an APK **you supply**, executes them in an embedded ARM JIT
([dynarmic](third_party/dynarmic/VENDORED.md), 0BSD) and replaces Android with host-side re-implementations of
the system libraries (bionic, a fake JVM, GLES, audio, windowing). Licensed under GPL-3.0-only.

Status: play mode works. With your 1.7.4 to 1.7.6 APK the game boots to the main menu, creates and loads worlds,
takes mouse and keyboard input, plays audio and can join a server over the network. It is a work in progress with
known issues (see [Known issues](#known-issues)). Architecture and milestones: [`docs/feasibility/PLAN.md`](docs/feasibility/PLAN.md).

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
| `bhpc-run` | runner: M0 fixture runner and benchmark, `--apk ... --init-only` (M1), and play mode with `--features play` |

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

### How to run (Linux)

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

The first start takes a few seconds to load the libraries. A consent dialog ("Enable multiplayer and free
crystals?") is answered with its first button ("Allow") automatically; `--alert-choice N` picks another one.
The window closes the run when you close it.

Useful flags (all optional):

| Flag | Effect |
|---|---|
| `--data-dir DIR` | where saves and settings live (see below) |
| `--window WxH` | window size, for example `--window 1280x960` |
| `--seconds N` | quit after N seconds |
| `--shot FILE.png` with `--shot-at N` | save a screenshot of swap N (and `FILE-first.png` of the first frame); `--exit-after-shot` quits afterwards |
| `--script FILE` | scripted input, see below; `--click X,Y@MS` is a one-off click at a time in ms; `--shot-dir DIR` is where script `shot` steps go |
| `--force` | run a build that is not one of the known versions |
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
[`docs/m7/net-bringup.md`](docs/m7/net-bringup.md).

**Controls** (`bhpc-input`, the game is a touch game and the mouse is the finger):

| Input | Effect |
|---|---|
| left button press, drag, release | one-finger touch (tap, drag, long press = hold) |
| right button drag | two-finger pan |
| mouse wheel | two-finger pinch (zoom) around the cursor |
| Ctrl + left drag | pinch around the press point (right/up = spread) |
| Esc | Android Back |
| letters, digits, arrows, Enter, Backspace, Delete, Tab, Space, Home, End | Android key events; while a text field has focus they type into it instead |
| F11 | toggle fullscreen (the picture is scaled to the screen, bars keep its aspect ratio) |
| close window | quit |

**Join a local server.** Start a server first (for example `blockheads_server --new myworld --port 15160`), then in the
game: Join World > Advanced... > Online IP/Port..., tap the port field and enter the server port (the field is
pre-filled with 15151), optionally set a nickname, then Join Online World. Details and a script that does this are in
[`docs/m7/net-bringup.md`](docs/m7/net-bringup.md).

**Where saves go.** Into the data directory: `--data-dir DIR`, or by default `$XDG_DATA_HOME/blockheads-pc`
(`~/.local/share/blockheads-pc`). Worlds and settings are under `DIR/home`, caches under `DIR/cache`, temporary
files under `DIR/tmp`; they persist between runs. Delete the directory to start from scratch.

**Launcher.** `app/` is the "Blockheads PC" launcher (Tauri 2 + SvelteKit): a setup wizard (game file, window and
graphics, controls, import Android saves), a Home screen that starts and supervises the game, and Settings. It keeps
its choices in `~/.config/blockheads-pc/launcher.toml` and starts the `bhpc-run` sidecar with `--config` pointing at
that file. See [`docs/launcher.md`](docs/launcher.md) for building it, the installers and what is not wired yet
(fullscreen, vsync, key bindings and the game window title/icon are saved but not applied by `bhpc-run` yet).
`bhpc-run --config launcher.toml` also works on its own; flags next to it win over the file.

**Load only** (no window, runs every constructor and prints the thunk histogram; exit code 0 = fine, about 2 s):

```sh
./target/release/bhpc-run --apk "$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" --init-only --data-dir /tmp/bhpc-data
```

Further reading: [`docs/m3/play-bringup.md`](docs/m3/play-bringup.md) (play mode internals),
[`docs/m7/net-bringup.md`](docs/m7/net-bringup.md) (networking). The smoke test runs the load path against a real APK;
it is `#[ignore]`d and needs the APK:

```sh
BHPC_APK="$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" cargo test -p bhpc-run --test smoke -- --ignored --nocapture
```

### Known issues

- Verified on x86_64 Linux only (Windows, macOS and arm64 are planned).
- Only the 1.7.4, 1.7.5 and 1.7.6 builds are known to boot.
- Performance is that of a JIT plus host re-implementations: expect the frame rate to be well below a phone's on
  busy scenes, and the first seconds of a start to be slow.
- A fresh data directory starts in a world scene a few seconds after the menu in the sanity run (first-run flow of
  the game); this has not been investigated.
- Some Java services are still stubs and answer 0/null (notifications, news client, in-app purchases, ad and
  analytics SDKs); the end-of-run report lists the ones the game called.
- No gamepad support, and the on-screen keyboard is replaced by host text input.
- The game picks its render size itself (for example 1024x768 in a 1280x960 window, 1334x750 in 1600x900) and the
  window scales that picture, keeping its aspect ratio (bars at the sides or top and bottom), also on high-DPI
  displays and in fullscreen. It chooses the size once at start-up: resizing the window afterwards scales and
  letterboxes the same picture instead of re-laying the game out.
- `bhpc-run --config` applies the launcher's APK, data directory, forced build and window size only; fullscreen, vsync
  and key bindings are saved by the launcher but not used yet (see `docs/launcher.md`).
- Gameplay has been exercised only briefly; expect rough edges and report what you find.
- Losing the window focus does not pause the game (it keeps running unfocused); minimising does, like
  backgrounding the app on a phone.

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
