# CLAUDE.md

blockheads-pc: run the user's own Blockheads Android build (ARM32 `.so` libs) on PC through a Rust loader,
an embedded ARM JIT (dynarmic) and host re-implementations of Android system libraries. GPL-3.0-only.
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
  layer (`Vm::ui_touch`, `control_screen_pos`) uses the turned portrait space (`screen_to_ui`).
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
  `__futex_wait/__futex_wake` use the same ticket queues as condition variables, keyed by the futex word address.
- `localtime`/`mktime`/`tzset` use the host zone via `HostClock::local_zone` (libc `localtime_r` on unix, UTC on
  Windows for now); `TZ=UTC` in the guest environment forces UTC.

## Gotchas learned in M7 (networking)
- Guest sockets live in `bhpc-run/src/vfd/net.rs` over non-blocking host sockets (unix only); details and the online
  join recipe in `docs/m7/net-bringup.md`. `BHPC_TRACE_NET=1` logs socket calls and datagram heads.
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
- The sidecar `bhpc-run` is built by `scripts/build-sidecar.sh` (static SDL2, separate target dir) into
  `app/src-tauri/binaries/` (gitignored); never put game files in a bundle (`scripts/check-bundle.sh`).
