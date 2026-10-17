# M1 bring-up, round 1

Goal (PLAN.md M1): `bhpc --apk <path> --init-only` maps all 25 libs, relocates them, runs every constructor, prints a
thunk histogram, and a host-driven smoke test calls into the ObjC runtime, CoreFoundation and `NSLog`.

## Status against the exit criteria

| Criterion | Result |
|---|---|
| All constructors return | **237 / 237** on 1.7.4, 1.7.5 and 1.7.6 (libApplication 158, libCoreGraphics 37, libFoundation 4, ... see the per-module table in the output). About 2.4 s wall time in a release build. |
| ObjC classes registered | `objc_getClass("UIApplication")`, `NSObject`, `NSString`, `NSMutableDictionary`, `UIView` are non-NULL; an unknown name is NULL (smoke test, all three builds). |
| `NSLog` output reaches the host log | `NSLog(@"...")` arrives as an `__android_log_print` record in the host sink (`E/blockheads: ...` on stderr in the CLI, a collecting sink in the smoke test). |
| Unimplemented imports hit during init | **0** (404 of 997 bound thunks are still traps, 381 of them GLES/EGL for M3). |
| Thunk histogram | Printed; top entries on 1.7.4: `strcmp` 384k, `strlen` 276k, `pthread_mutex_lock/unlock` 60k each, `memset` 16k, `memcmp` 15k. 352 distinct thunks called. |

Relocations applied: 262313 (1.7.4), 262276 (1.7.5), 262286 (1.7.6), as in the census. Cargo coverage gate
(`scripts/coverage.sh`, whole workspace): 100 % of lines and functions (27.4k regions, 99.0 %).

## How to run

```
cargo build --release -p bhpc-run
target/release/bhpc-run --apk ~/Downloads/"The Blockheads_1.7.4_APKPure.apk" --init-only --data-dir /tmp/bhpc-data
BHPC_APK=... cargo test -p bhpc-run --test smoke -- --ignored --nocapture
```

Diagnosis flags: `-v` (log every constructor), `--trace-thunks`, `--trace-fs` (paths and `getenv` names the guest asks
for), `--trace-jni`, `--trace-pc FILE` (single-step trace), `--watchdog CYCLES`, `--keep-going`, `--force` (unknown
build), `--package`, `--top N`. A stop prints the reason, r0-r15, and a symbolised guest backtrace (`.ARM.exidx` unwind
with dynsym names; a stop in a host function is listed as `<host name>` followed by the guest caller).

## What had to be built (in the order the guest asked for it)

1. **Integration** (`bhpc-run/src/apkrun.rs`): `Session` = APK + linker + bionic + CPU + fake JVM; `init_only` and the
   report. The M0 `hle.rs` was dropped in favour of `bhpc-bionic`.
2. **Backtraces** (`bhpc-loader/src/unwind.rs`, `Cpu::stop_regs`): EHABI compact/generic unwinder.
3. **libm** (68 functions via the host libm) and **POSIX surface** (`bionic/sys.rs`): std descriptors are real, files
   go through a **virtual file system** (`bionic/vfs.rs`): `/proc/<pid>/cmdline` (the package name), `/proc/cpuinfo`,
   `/dev/{null,zero,urandom}`, plus host directories mounted into the guest (`/data/data/<pkg>`, `/sdcard`, and the user's
   APK read-only at `/data/app/<pkg>-1.apk`). `qsort`/`bsearch` (nested guest calls), `setjmp`/`longjmp`, `fnmatch`,
   `dirname_r`, file `mmap` (private copy), `munmap` with page granularity (jemalloc trims mappings), `strftime`.
4. **stdio on files** (`fopen`/`fread`/`fgets`/`fseek`/...) and **`funopen`** (libv replaces stdout/stderr with a
   callback stream: output then flows through a guest callback).
5. **Threads** (`bionic/sched.rs`, `pthread.rs`): libSystem's constructor starts a helper thread and waits for it, so
   `pthread_create` is real. Each guest thread is a host thread with its own JIT but only the holder of a baton runs
   (deterministic, no data races in host functions). Mutexes, condvars (timed/untimed), rwlocks, join/detach/exit,
   per-thread `errno` and TLS (with destructors), sleep and `sched_yield` hand the baton over; "everything is blocked"
   is detected and stops the guest with a message instead of hanging.
6. **Fake JavaVM/JNIEnv** (`bhpc-jni`): all 233 `JNIEnv` slots and the 8 `JavaVM` slots exist; classes, member ids,
   strings, arrays and `RegisterNatives` are recorded, calls into Java are counted and answer 0/NULL. The recorded
   state (classes touched, natives registered, Java methods attempted) is the work list for M3.
7. **Java-loader emulation**: libv wraps file I/O and needs `libv_init` plus the `LibraryLoader.setenv` variables
   before anything else runs; the loop in `Session::run_constructors` runs `JNI_OnLoad(vm)` and `libv_init` right after
   libv's constructors, and `FsConfig::env` supplies `PACKAGE_*`, `CFProcessPath`, `HOME`, `TMPDIR`, `LANGUAGES`, ...
8. **JIT fixes found on the way** (`bhpc-cpu` shim): blocks are capped at 256 instructions and an all-zero word stops
   the guest (see CLAUDE.md); both avoid a host hang in dynarmic's translator.

## Open issues / next steps

- 404 imports are still traps: 381 GL/EGL (`bhpc-gles`, M3), plus `inet_*`, `sscanf` family, `strptime`, `dl*`, `__isinf`
  (needs the linker inside bionic), `__cxa_begin_cleanup`/`__cxa_type_match`, `__futex_wait/wake`, `__get_h_errno`,
  `AndroidBitmap_*`. None is hit during init; the unimplemented-import trap names the first one that is.
- JNI is permissive, not real: 23 `CallStaticObjectMethod` (class loading) return NULL, which makes BridgeKit log
  15 "Unable to register bridge method ..." lines (`wait`, `notify`, `hashCode`, `getClass`, AdvertisingId ...).
  M3 has to answer `ClassLoader.loadClass` and friends.
- Sockets, `epoll`, `poll`, processes: stubs that fail with a plausible `errno` (ENOSYS / EAFNOSUPPORT). libSystem's
  helper thread starts and parks in `pthread_cond_timedwait`; real I/O multiplexing is M5 (multiplayer).
- The scheduler never preempts: a thread that spins on memory without a blocking call starves the others. A tick-based
  yield (the CPU already counts cycles) would fix that if it shows up.
- `HaltExecution` cannot interrupt a cycle-counted tight loop (see CLAUDE.md); a wall-clock watchdog is therefore not
  available, only the cycle budget.
- Host `libm` is used for `erf`, `lgamma`, `tgamma`, `logb`, `remainder`, `remquo`; results can differ in the last
  bit from the device's libm. Windows/macOS link these from the system C runtime (not yet built there).
- `fseek`/`ftell` do not work on `funopen` streams (no `_seek` callback support); no `freopen`, `fscanf`/`sscanf`.
- Region coverage is 99.0 % (the gate is lines and functions); the misses are `?` error edges.
- G0 housekeeping still open: no CI workflow (only `scripts/coverage.sh`), README prerequisites, crate rename
  `bhpc-loader` -> `bhpc-elf`, census data hygiene.
