# CLAUDE.md

blockheads-pc: run the user's own Blockheads Android build (ARM32 `.so` libs) on PC through a Rust loader,
an embedded ARM JIT (dynarmic) and host re-implementations of Android system libraries. GPL-3.0-or-later.
Architecture and milestones: `docs/feasibility/PLAN.md` (read first), `docs/feasibility/C-execution-strategy.md`.
Umbrella rules: `../AGENTS.md`.

## Rules
- Never commit or ship game files: no APKs, extracted `.so`, assets, decompiled code. Test APKs live in `~/Downloads`
  and are extracted only into the session scratchpad, never into the repo. Only our own fixtures are committed.
- Rust via rustup stable, Conventional Commits, branch `dev`. Do not push without being asked.
- 100% coverage for our own Rust crates (`cargo llvm-cov`); vendored `third_party/` and the C++ shim are excluded.
  Report real numbers and list what is not covered.
- `third_party/dynarmic` is a verbatim pinned copy (see `third_party/dynarmic/VENDORED.md`); do not edit it, patch in
  the shim instead.

## Test gates (agents: use these, not ad-hoc cargo commands)
- Every commit: `scripts/check-fast.sh` (changed crates only: fmt, clippy, coverage 100 % via `COVERAGE_PACKAGES`,
  doctests, tests of dependent crates, app vitest if `app/` changed; ~1 min). `--dry-run` shows the selection.
- Before merging into `dev`: `scripts/check-full.sh` (workspace fmt/clippy with `play`, `scripts/coverage.sh`, doctests,
  app check + vitest + Playwright in parallel, then `scripts/smoke-game.sh`, a ~35 s headless boot -> world -> menu run).
  Never merge with a red full gate; no multi-round benchmarks in gates.
- Headless game runs: `env -u WAYLAND_DISPLAY -u DISPLAY SDL_VIDEODRIVER=x11 GDK_BACKEND=x11 SDL_AUDIODRIVER=dummy`
  under `dbus-run-session` + `xvfb-run`, temp HOME. `SDL_VIDEODRIVER=x11` is required: without it SDL falls back to
  `$XDG_RUNTIME_DIR/wayland-0` and the window opens on the developer's desktop.
- Worktrees share build caches: dynarmic archives in `~/.cache/bhpc/dynarmic-<hash>` (reused by every profile and
  checkout, `BHPC_DYNARMIC_CACHE=0` off) and sccache for crates.io deps when installed. Keep one `target/` per
  worktree (no shared `CARGO_TARGET_DIR`: cargo's lock would serialise agents). Run the scripts with `TMPDIR` on a disk
  with space if `/tmp` (tmpfs) is full.
- Playwright: `PW_PORT` picks the preview port (check-full uses a free one) so parallel worktrees do not collide.

## Gotchas learned in M0
- dynarmic forbids nested `Run()` on one `Jit`; host functions that must call guest code use `Cpu::tail_call`
  (thunk stubs return through `r12`).
- The JIT calls back into Rust through a raw pointer: always pass the owner to the FFI `run`/`step` (`escape`
  argument) so rustc does not cache fields across the call (this once made stops invisible in release builds).
- `strex` needs the `MemoryWriteExclusive*` callbacks; without them every store-exclusive fails and ARM spinlock
  loops (libc++abi refcounts) hang forever.
- Debug aids: `--trace-thunks`, `--watchdog CYCLES` (reports pc on expiry), `--trace-pc FILE` (single-step trace).

## Gotchas learned in M1 bring-up
- `bhpc-run --apk <apk> --init-only [--data-dir D] [--force] [-v] [--trace-thunks] [--trace-fs] [--trace-jni]
  [--trace-pc FILE] [--watchdog CYCLES]` loads the 25 libs, runs every constructor and prints the thunk histogram;
  see `docs/m1/bringup-round1.md`. The real-APK smoke test is ignored by default:
  `BHPC_APK=... cargo test -p bhpc-run --test smoke -- --ignored --nocapture`.
- Stop registers (`Cpu::stop_regs`) are only exact for stops at block boundaries (thunks, exceptions). For a
  guest fault mid-block use `--trace-pc` (single-step) and read the last lines.
- dynarmic translates a *block* per entry and rescans it for every conditional instruction: a long run of
  conditional instructions is quadratic. The shim therefore stops a block after 256 instructions and turns an
  all-zero instruction word (`andeq r0,r0,r0`, i.e. a jump into zeroed memory) into an undefined-instruction
  stop. Without that, a jump into bss hangs the host in the translator instead of stopping.
- Cycle counting makes `b .` uninterruptible by `HaltExecution` (LinkBlock only checks the cycle budget):
  use the per-call cycle budget (`--watchdog`) as the way to stop runaway guests.
- A panic inside a thunk cannot unwind through the JIT frames: it aborts the process ("failed to initiate
  panic"). Thunks must not `unwrap()` on guest-controlled values; tests that need a failing worker request a stop.
- Guest threads are real host threads but run one at a time (baton in `bhpc-bionic/src/sched.rs`); every host
  function must be a *shared* thunk (they all are via `bhpc_bionic::builtin`). Blocking calls go through
  `sched::block`, never through `std::sync` waits while holding the State lock.
- The Java-side loader is emulated in `bhpc-run/src/apkrun.rs`: environment variables come from
  `bhpc_vfs::GuestPaths::env`, and after each library's constructors `JNI_OnLoad(vm)` and (libv only) `libv_init` run.
- File layer: `bhpc-vfs` owns paths, mounts, `/proc` `/dev` content and the Android path map; `bhpc-bionic/src/vfs.rs`
  is only the guest fd table over `Arc<dyn FileSystem>` (`Config::fs`). `--data-dir D` is a `bhpc_vfs::DataDirs`
  root (home/, cache/, sdcard/, ...); without it the per-user default is used. The fake JVM is `bhpc-jni`
  (`bhpc_jni::install`): lenient, so unknown Java classes/members are invented and counted, calls answer 0/NULL.
- Never `lock(st)` twice in one expression/`match` scrutinee (the fake JNI deadlocked once that way).
- libv's `__wrap_opendir` does not call our `opendir`: it `open`s the directory (O_DIRECTORY), builds a bionic-layout
  `DIR` (fd in word 0, entry buffer at +16) in its own memory and calls the libc `readdir`/`readdir_r` on it. Our
  `readdir` must therefore accept any `DIR` by its first word; when it did not, the world list (Foundation's
  `contentsOfDirectoryAtPath:` on `Library/Application Support/saves`) came back empty and no saved world showed.
  `BHPC_APK=... cargo test -p bhpc-run --test saves_apk -- --ignored` checks it on the real libv.

## Gotchas learned in M3 (play mode)
- `bhpc-run --apk X` without `--init-only` needs `--features play` (SDL window); notes and flags in
  `docs/m3/play-bringup.md`. Everything with logic lives in modules that are covered without a display (`dl`, `vfd`,
  `mmapio`, `tracks`, `input`, `png`); only `play.rs` (SDL, threads) is excluded by the feature.
- The game's main thread is the guest main thread (libv records it for `pthread_main_thread_np`); the Android UI
  thread is a guest thread. Lifecycle natives queue blocks on the main run loop, so never call them on the main thread.
- Thunks pre-registered before linking win over bionic's (`Hle::resolve` looks at `cpu.thunk_addr` first): that is how
  GL/EGL, OpenSL ES, `dlopen`, pipes/epoll, `pread`/`pwrite`/`mmap(MAP_SHARED)`, `getrlimit`
  are provided without touching `bhpc-bionic`.
- `sched::sleep`/`sched::block` give the baton to other guest threads; never use `std::thread::sleep` in a thunk that
  other guest threads depend on.
- The fake VM allocator must return zeroed memory; `BHPC_JNI_CALLS`-style tracing is `--trace-jni --trace-jni-calls`.

## Gotchas learned fixing the click freeze and window scaling
- Native touches (`Window.nativeTouchesBegin/...`) take landscape screen pixels like the window; only the Java view
  layer (`Vm::ui_touch`, `control_screen_pos`) uses the portrait space centred on the window (`screen_to_ui`, `ui_offset`).
- libSystem's kqueue/dispatch go through `syscall(351/350/353/354)`; `vfd` answers them. A thunk registered later under
  the same name (`mmapio`'s `write` watcher) replaces the earlier one: wrappers must forward to the table's own fds.
- The game chooses its content size once; the window letterboxes it (`vsurface.rs`); `Geometry` maps the mouse.
- Focus loss must not send `WillResignActive` (the game pauses). Debug with `--trace-fn` on libSystem exports such as
  `dispatch_after`, `_dispatch_mgr_wakeup`, `linux_evfilt_user_knote_modify`.

## Gotchas learned in M2 (G1 fixes)
- Guest threads run one at a time under the scheduler baton and only hand it over when they block. A spinning
  thread therefore needs the cycle time slice: `Cpu::set_slice` / `sched::enable_time_slice` (the CPU must count
  cycles and have a budget). Whoever builds the main CPU (`bhpc-run`) has to call it for the main thread; threads
  made with `pthread_create` inherit it. `sched::set_wall_deadline` is the host watchdog checked at slice ends.
- `pthread_once` is IN_PROGRESS while the initialiser runs; other threads block in `sched::block` until it returns.
- Waits are keyed (`sched::block_on(addr)` / `notify_key(addr)`; `block`/`notify` = anything): a waker only wakes
  the waiters of its object. Guest threads run in parallel by default (`docs/perf/parallel-guest-threads.md`;
  `--serial-guest` / `BHPC_PARALLEL_GUEST=0` / the launcher's "Run game threads in parallel" off bring back the
  baton): the baton becomes a host lock held only in host code; thunks listed in `bhpc_bionic::gate_free` run
  without it, so they must do each check-and-update in one `lock(st)` section and wait with `Sched::key_seq` + `block_seq`.
  `__futex_wait/__futex_wake` use the same ticket queues as condition variables, keyed by the futex word address.
- Parallel threads expose guest races the baton hid. Apportable's mk_timer receive slept until a re-armed deadline
  (seconds of input lag on the UI thread); `bhpc-run/src/mktimer.rs` patches it. To find such stalls, time the
  blocking thunks (`watch_blocking` in play.rs) per thread and look at the caller's stack for `usleep`/cond waits.
- `localtime`/`mktime`/`tzset` use the host zone via `HostClock::local_zone` (libc `localtime_r` on unix, the CRT's
  `localtime_s` on Windows); `TZ=UTC` in the guest environment forces UTC.

## Gotchas learned in M7 (networking)
- Guest sockets live in `bhpc-run/src/vfd/net.rs` over non-blocking host sockets on every host; details and the
  online join recipe in `docs/m7/net-bringup.md`. `BHPC_TRACE_NET=1` logs socket calls and datagram heads. The guest
  speaks the Linux ABI: never pass its numbers to the host directly, translate through `vfd/net/abi.rs` (tables for
  Linux, macOS and Windows, tested on every host); `net/sys_unix.rs` / `net/sys_windows.rs` are the only host calls.
- Anything that rebinds `read`/`poll`/`select` after `vfd::install` silently bypasses the descriptor table
  (`watch_blocking` did): register wrappers first, the table last.
- The game runs input handlers on the Android UI guest thread; its CFRunLoop is only serviced by the UI loop's
  `CFRunLoopRunInMode(..., 0, true)` call, so `performSelector:afterDelay:` from a button works.

## Launcher (app/, crates/bhpc-launcher)
- `app/` is a Tauri 2 + SvelteKit app (bun); `app/src-tauri` is its own cargo workspace and a thin layer over
  `crates/bhpc-launcher`, where all logic lives and is covered at 100 % by `scripts/coverage.sh`. See `docs/launcher.md`.
- Interface: `bun run check`, `bun run test` (vitest, 100 % statements/branches/functions/lines), `bun run test:e2e`
  (Playwright against the mock backend in `app/e2e/mock.ts`). Svelte compiles `text {x}` mixes into `x ?? ''`:
  write `{`text ${x}`}` when `x` is never nullish, or the branch stays uncovered.
- JSON between Rust and TS is snake_case (serde defaults); `LaunchConfig` is the same struct as `launcher.toml`.
- `bhpc-run --embed|--progress` prints `@progress <phase> <done> <total>` / `@ready` lines on stdout (protocol and
  loading screen in `docs/launcher.md`, code in `bhpc_launcher::progress`); the launcher keeps them out of the log.
  `tauri dev/build` run `scripts/ensure-sidecar.mjs` first so the bundled sidecar is never stale.
- The sidecar `bhpc-run` is built by `scripts/build-sidecar.sh` (static SDL2, separate target dir) into
  `app/src-tauri/binaries/` (gitignored); never put game files in a bundle (`scripts/check-bundle.sh`).

## Gotchas learned: input thread and the "frozen" world
- Touch, key and text natives (`Window.nativeTouches*`, `nativeKey*`, `TextField.*`) must run on the guest main thread
  (t0, the only one with a GL context: shader/texture creation from `tap:`/`updatePreviewView` fails elsewhere). The UI
  thread posts them to `bhpc-run/src/mainq.rs`; `Gles::set_frame_hook` drains the queue inside `eglSwapBuffers` on t0.
  Lifecycle/surface/focus natives stay on the UI thread. Java control events (`Control.event`, button presses) and alert
  answers (`nativeOnClick`) run the game's UIKit actions, so they go through the same queue; before the first frame a
  waiting poster kicks the main run loop (`dispatch_async_f` on the main queue). On the UI thread the start-up alert's
  `loadGameView` raced `initOpenGL` (NaN projection, black world; `docs/perf/parallel-guest-threads.md`).
- A world that looks frozen headlessly is usually the game's modal tutorial popup ("WELCOME TO THE BLOCKHEADS", built
  from `com.apportable.ui` Java views that the port does not draw): it calls `setPaused:`, hides the HUD and stops the
  simulation. `--script` `clicktext OK` dismisses it; use `dumpviews` to see such invisible views.

## Gotchas learned: drawing the Java views
- The `com.apportable.ui` views (popups, dialogs) are rendered by `Vm::render_ui` (`bhpc-jni/src/render.rs`): software
  rasteriser (fills, bitmaps, text via `ab_glyph`) into a window-sized premultiplied RGBA frame, cached by item list
  (`UiFrame::serial`). `Hooks::before_swap` (play.rs) hands it to the window through `OverlayFeed` and
  `GlWindow::set_overlay`; `vsurface.rs` blends it over the letterboxed game surface inside `compose` (so screenshots
  include it) and restores all GL state it touches. Nothing visible (menus, HUD) means no overlay and no cost.
- Java `Bitmap`s are fake objects whose pixels (premultiplied RGBA) live in guest memory (`jlib/graphics.rs`); the
  native side fills them through `AndroidBitmap_lockPixels` (thunks registered by `Vm::install`) or
  `copyPixelsFromBuffer`. They are never freed (small, a few per popup).
- The views stay upright: nothing tells the game the device turned, so Apportable never rotates the portrait
  768x1024 space; `Vm::ui_offset` centres it on the 1024x768 window (offset (128, -128)) for drawing and for
  `ui_touch`, and a fill that covers the whole space (the dimmer) is stretched over the window.
- Text: labels use `TextStyle` (font asset `BlockheadsFont-Regular.otf` from the APK, size in px, colour, alignment,
  lines, shadow); missing/unparsable fonts fall back to the bundled Liberation Sans. `dumpviews` shows bg colour, clip,
  bitmaps (`WxH:opaque-pixels`) and the text style.

## Gotchas learned: JIT tuning and guest HLE (perf/jit-hle)
- `CpuOptions::fastmem_exclusive` (ldrex/strex inline through fastmem, the global monitor still arbitrates) and
  `fast_fp` (dynarmic `Unsafe_InaccurateNaN | Unsafe_IgnoreStandardFPCRValue`: NaN payloads may differ, NEON honours
  the guest FPSCR instead of forcing flush-to-zero/default-NaN) are on by default; `BHPC_JIT_SAFE=1` turns both off.
  Never enable `Unsafe_IgnoreGlobalMonitor` (guest threads share the monitor).
- Thunk dispatch (`tramp::svc`) reads the handler from a lock-free slot array (`ThunkTable::slots`): no RwLock or
  `Arc` clone per call, which matters once guest threads run in parallel. Rebound handlers stay alive in the table.
- libgcc's integer division helpers in `libv.so` (`__aeabi_uidiv`, `__aeabi_idivmod`, `__modsi3`, ...) are redirected
  to `udiv`/`sdiv` routines (`bhpc-run/src/fastdiv.rs`) after linking; zero divisors still take the helper's own path.
  `BHPC_GUEST_HLE=0` disables it. Differential check against the real libv:
  `BHPC_APK=... cargo test -p bhpc-run --test fastdiv_apk -- --ignored`.
- Profiling: `PERF_BUILDID_DIR=DIR` makes dynarmic write `DIR/perf-PID.map` (one line per translated block,
  `a32_a<PC>_...`), usable with `perf report` or any sampler that records host RIPs. libc/libm/malloc are already host
  thunks; guest-side hot spots are diffuse (objc_msgSend ~3%, zlib/png/Skia during loading), and a thunk round trip
  (~25 ns) costs more than objc_msgSend's cache-hit path, so it stays guest code.

## Gotchas learned porting to Windows (and every non-Linux host)
- CI for the other five targets is `.github/workflows/ci-crossplat.yml` (whole workspace, JIT tests serially first);
  nothing local runs Windows. `BHPC_SKIP_NATIVE=1 cargo check --workspace --all-targets --target
  x86_64-pc-windows-msvc` (or `aarch64-apple-darwin`) type-checks the foreign `cfg` code without its C++ toolchain.
- dynarmic's own Windows fastmem handler crashes (no x64 shadow space before its C++ call); the shim builds
  `shim/exception_handler_windows.cpp` (a vectored handler, x64 + arm64) into dynarmic instead (cmake/CMakeLists.txt).
- Page protection is `GuestMemory::protect` (mprotect / VirtualProtect); never call `libc::mprotect` elsewhere.
- No OpenGL ES on the host (macOS, some Windows drivers): `bhpc-app/src/sdl.rs` falls back to a desktop
  compatibility context and `bhpc_gles::glsl::to_desktop` translates the shaders. `BHPC_GL=desktop` forces that path
  on Linux too (the headless smoke run passes with it).
- Tests must not assume Linux paths or layouts: `/proc`, `/bin/sh` (Git's `sh` is on the Windows runners' PATH),
  `file:///C:/...` URLs, host pointers above the guest window, `\` separators in displayed paths.

## ObjC method hooks from the host (mod system foundation)
- `bhpc-run/src/objchook.rs`: `ObjcHooks::hook(Target, Option<Sig>, handler)` + `install(cpu, &Runtime)` swizzle a
  guest method through Apportable's own runtime exports (`method_setImplementation` with a host thunk as the IMP):
  no code patch, no JIT flush, ARM and Thumb IMPs alike. Handlers run under `catch_unwind` (a panic falls back to the
  original) and hold the host gate like any thunk. Before hooks tail-call the original; `call_original` (after /
  result changes) runs it on the nested child JIT. Signatures are ObjC type encodings checked against the runtime's.
- Install after the constructors; `class_getInstanceMethod` realises a class without `+initialize`. Unknown classes
  or methods stay waiting until a later `install`. Module docs list the limits.
- Dev proof: `BHPC_DEV_OBJC_HOOKS=1` (play build) logs `-[BHClient match:didReceiveData:fromPlayer:]` packets, times
  `-[WorldUI render:...]` (after hook) and samples `-[World update:...]` (before hook); `BHPC_THUNK_STATS=1` adds
  per-thunk host time. Overhead: `cargo test --release -p bhpc-run --lib objchook_overhead -- --ignored --nocapture`.
