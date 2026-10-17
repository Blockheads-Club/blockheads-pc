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
