# G1 gate decision (main go/no-go)

**Date:** 2026-10-18
**Reviewer:** G1 gate reviewer (workflow agent)
**Inputs:** M1 bring-up, commits `895ddd5`..`c7dc5ab` on `dev`, not pushed; `docs/m1/bringup-round1.md`.

## Verdict: PASS-WITH-FIXES (proceed to M2)

The G1 rule in `docs/feasibility/PLAN.md` §3 says: if M1 slips more than 2 weeks past plan because of the ObjC runtime,
EHABI or jemalloc, re-estimate everything at x2 or stop. **M1 did not slip.**

- It was planned at 3 dev-weeks, plus about 1 week of G0 fixes.
- The reviewer re-ran every exit criterion and all of them are met on all three builds.
- None of the three named risks caused real trouble:
  - **ObjC runtime:** it runs as unmodified guest code in libSystem. Class registration needed nothing beyond the bionic surface.
  - **EHABI:** guest C++ unwinding already worked in M0. The host-side `.ARM.exidx` unwinder used for backtraces was built here, ahead of R7.
  - **jemalloc:** it needed only page-granular `munmap` and file `mmap`.
- Work was pulled forward from M2: real guest threads, mutexes, condition variables, TLS, `setjmp`/`longjmp` and `qsort`/`bsearch` through nested calls.

The x2 re-estimate is not triggered. The fixes below are housekeeping left over from G0, plus a few correctness items found in review. Together they take under 1 week and go into the first week of M2. None of them blocks the approach.

## M1 exit criteria, re-verified by the reviewer

All runs were on Linux x86_64 with the release build `target/release/bhpc-run --apk <apk> --init-only --data-dir <scratch>`. The APKs were read in place from `~/Downloads`.

| Criterion (PLAN.md M1) | 1.7.4 | 1.7.5 | 1.7.6 | Result |
|---|---|---|---|---|
| Libraries mapped (DT_NEEDED order) | 25 (+9 host libs) | 25 | 25 | **Met** |
| Relocations applied (census predicts about 262k) | 262,313 | 262,276 | 262,286 | **Met** |
| All constructors return | 237/237 (libApplication 158) | 237/237 | 237/237 | **Met** |
| Thunk-call histogram printed | 352 distinct thunks called | yes | yes | **Met** |
| Unimplemented imports hit during init | **0** (404 of 997 bound thunks are traps) | 0 | 0 | **Met** |
| Process exit code | 0 | 0 | 0 | **Met** |
| Smoke test (`BHPC_APK=... cargo test -p bhpc-run --test smoke -- --ignored`) | ok (19.3 s) | ok (16.0 s) | ok (7.4 s) | **Met**. Debug build, timed while the coverage job was running. |

What the smoke test asserts, read from `crates/bhpc-run/tests/smoke.rs`:
- There are no constructor failures, and the unimplemented-hit list is empty.
- `objc_getClass` returns non-NULL for `UIApplication`, `NSObject`, `NSString`, `NSMutableDictionary` and `UIView`, and NULL for an unknown name.
- `CFStringCreateWithCString` gives `CFStringGetLength == 19`, and `CFStringGetCString` round-trips the text.
- `NSLog(@"...")` output shows up in the host log sink.

Other evidence, re-run by the reviewer:

| Check | Result |
|---|---|
| `cargo test --workspace` | Green: 282 passed, 0 failed, 2 ignored (bionic 130, cpu 46, jni 11, loader 52, mem 16, run 7+8+11+1). **Re-run 5 times with 0 failures.** |
| `cargo clippy --workspace --all-targets` | 0 warnings, 0 errors |
| `scripts/coverage.sh` (whole workspace, `--fail-under-lines 100 --fail-under-functions 100`) | **Exit 0. Lines 10,992/10,992 (100%), functions 1,239/1,239 (100%), regions 99.01% (272 missed, all `?` error edges).** Exclusions: `third_party/`, the C++ shim (not Rust), test files and `testelf.rs`, as AGENTS.md allows. |
| Release init wall time, 1.7.6, machine otherwise idle | 2.03-2.12 s over 3 runs. Runs taken while the coverage build was compiling were 2.9-6.3 s. |
| Guest threads after init | 1 created, 1 alive. libSystem's helper thread is parked in `pthread_cond_timedwait`. |
| Fake JNI after init | 5 classes, 1 method ID, 0 field IDs, 4 natives registered. Java calls: 23 with method id `?` (see F5) and 1 `Class.getClassLoader`. BridgeKit logs 15 "Unable to register bridge method" lines. |
| Repo hygiene | `git status` is clean. Nothing from the APKs is in the tree; `.gitignore` covers `*.apk` and extracted dirs. `KNOWN_BUILDS` holds only libApplication SHA-256 identifiers. |

### Effort against plan

M0, G0 fixes and M1 were planned at 4.5 + about 1 dev-weeks. The commit history runs from 2026-10-17 22:09 (census) to 2026-10-18 01:15 (last M1 fix). Wall-clock time cannot be converted to dev-weeks one-to-one, because the work was agent-driven. Two facts still stand: M1 is complete with no scope cut, and nothing in it hit a blocker of the kind the gate guards against. **Slip: none.**

## Findings from the review

| # | Finding | Severity | Where it goes |
|---|---|---|---|
| F1 | **G0 fix 1 is only half done.** `scripts/coverage.sh` gates at 100%, but there is no CI workflow (no `.github/`). The AGENTS.md rule says coverage must fail *CI*. | Required | M2 week 1 |
| F2 | **The G0 fix 2 load-time assert is missing.** The undefined/unpredictable-instruction stop reason exists (`Stop::Unpredictable`), but nothing checks that `OPENSSL_armcap_P` is still 0 after `.init_array`. A grep for `armcap` in `crates/` finds nothing. | Required (small) | M2 week 1 |
| F3 | **The README is stale.** It says "Status: M0" and lists `bhpc-bionic` and `bhpc-jni` as empty stubs. The G0 fix 4 build prerequisites (Boost, CMake >= 3.16, C++20, the dynarmic rebuild of about 3.5 min per profile) and the `--init-only` usage are missing. | Required | M2 week 1 |
| F4 | **Census data hygiene (G0 fix 6) is undecided.** `docs/m0/opcode-census-unsupported.csv` (247 rows) and `-odd.csv` still carry an `encoding_hex` column of raw game instruction words. Decide in writing, or drop the column. The reviewer recommends dropping it: address plus mnemonic is enough for the dead-code guard, and the hash-gated patch model only needs words at patch sites. | Required (decision) | M2 week 1 |
| F5 | **Permissive JNI hides failures.** The 23 calls labelled `?` carry method id 0, because `FromReflectedMethod` is stubbed to return 0. On a real VM they would crash. PLAN.md §1.4 asks for unimplemented slots to log loudly. Add a `--jni-strict` mode that stops on a call with id 0 and on abort-class slots, and add the guest caller to the `?` bucket. | Should | M2 (strict switch); M3 (real reflection) |
| F6 | **`pthread_once` is not thread-safe.** The state flips to DONE before the init routine returns. With the baton scheduler that is benign only while init never blocks. libdispatch's `dispatch_once` and `pthread_once` in M2 tests can block inside init. Implement the PLAN.md design: a CAS on the guest word with an IN_PROGRESS state and waiters. | Required | M2 |
| F7 | **The scheduler never preempts.** Guest threads run one at a time under a baton and only hand it over on blocking calls. M2 (libdispatch workers), M3 (MainThread plus Looper) and M4 (GLThread) bring many threads, and ARM spin-waits (`ldrex`/`strex` loops, `OSSpinLock`) will starve. Add a cycle-budget time slice: the CPU already counts cycles, and on expiry it yields the baton. Measure the cost of serialised execution before M4; parallel execution is a later option. | Required | M2 |
| F8 | **The guest clock is UTC.** Guest `NSLog` timestamps read 22:16 while the host showed 01:16 +03. `TZ` and `localtime` are not plumbed from the host. | Minor | M2 |
| F9 | **OpenAL's OpenSL ES backend initialises during init** (the log shows `alc_opensles_init`). `dlopen`/`dlsym` are still trap thunks. The first device open will call `dlopen("libOpenSLES.so")`, and §1.6 needs that to return NULL so OpenAL falls back to AudioTrack. Implement `dl*` inside bionic over the loader's module table. | Required by M3/M6 | M2 |
| F10 | `Class NSBlock is implemented in two different images` is logged during init. It is probably benign (libSystem and Foundation both define it). Confirm which image wins, because block copy and release go through that class. | Minor | M2 (investigate) |
| F11 | **There is no wall-clock bound on the guest.** dynarmic's `HaltExecution` cannot break a cycle-counted `b .`, and the translator has no time bound. The 256-instruction block cap and the zero-word guard are mitigations. The F7 time slice also gives a periodic exit point that a host watchdog can use. | Should | M2, together with F7 |
| F12 | Housekeeping from G0 that is still open: rename `bhpc-loader` to `bhpc-elf` or update PLAN.md (pick one); `tools/census/README.md` exists, so add a line saying it is outside the coverage gate. | Minor | Any time |

## Revised estimate (M2-M9)

What M1 taught us:
1. The "run the bundled runtime, emulate only the OS boundary" bet holds. The ObjC runtime, CF, Foundation, ICU, jemalloc, BridgeKit's constructors and OpenAL all initialise as unmodified guest code.
2. The imported surface is as small as analysis A said: 997 bound thunks, of which 352 are used during init.
3. The three builds behave identically. That makes M9's version work cheaper.
4. Threads arrived early (libSystem starts a helper thread in its constructor). Part of M2 is therefore done, but the scheduler needs preemption.
5. The JNI side is where the unknowns are, as R1 predicted. The first real BridgeKit traffic is class loading through `ClassLoader.loadClass` and reflection (`getClass`, `FromReflectedMethod`), and none of it works yet.
6. dynarmic needed two shim guards (the block cap and the zero-word stop), and it cannot be halted out of a tight loop. Debuggability (R7) is acceptable thanks to the EHABI backtraces and the single-step trace.

| # | Milestone | Plan | Revised | Why |
|---|---|---|---|---|
| — | G0/G1 fixes (F1-F4, F12) | — | 0.5 | CI workflow, armcap assert, README, census column, rename |
| M2 | bionic core and threads | 3.5 | **3.0** | -1: pthread, TLS, condvars, `setjmp` and nested calls are done. +0.5: preemptive time slice (F7/F11), real `pthread_once` (F6), `dl*` (F9). Still to do: virtual fd table, `epoll`/`poll`/`select`, pipes and eventfd, sockets over `mio`, `__futex_*`, the `sscanf`/`strptime`/`inet_*` family, `funopen` seek, the printf differential test (NDK r27 is installed at `~/Android/Sdk/ndk/27.2.12479018`), and the libdispatch, CFRunLoop and zzip demos. |
| M3 | Fake JVM and `UIApplicationMain` | 3.5 | **4.0** | +0.5: the first BridgeKit traffic is reflection and class loading (`loadClass`, `getClass`, `FromReflectedMethod`, `Method`/`Class` objects). That is earlier and deeper than B assumed, so the object model needs `java.lang.reflect` from day one. R1 is unchanged. |
| G3 | Gate | — | — | — |
| M4 | First frame (Linux) | 3.5 | **3.5** | No new information. Serialised threads (MainThread plus GLThread) may cost frame time, so measure that early in M4. |
| M5 | Interactive menu | 2.5 | 2.5 | — |
| M6 | Gameplay and audio | 5.0 | 5.0 | — |
| M7 | Multiplayer | 1.5 | 1.5 | Socket groundwork moves into M2 (it was already counted there). |
| M8 | Windows and macOS | 4.0 | **4.5** | +0.5: Windows and macOS builds of the shim, dynarmic and the host `libm` functions (`erf`, `lgamma`, `tgamma`, `logb`, `remainder`, `remquo` fall through to the host C runtime) were not tried. The baton scheduler and real host threads must be checked against macOS `MAP_JIT` per-thread W^X. |
| M9 | Versions and polish | 3.0 | **2.5** | -0.5: 1.7.5 and 1.7.6 already pass M1 unchanged, and everything binds by name. |
| | **Remaining total** | **26.5** | **27.0 (range 22-37)** | Effectively unchanged. The first frame is still about 11 dev-weeks after G1. |

The x2 factor is **not** applied, because the G1 trigger did not fire. The range narrows at the low end because M1's scope landed without surprises. The high end keeps R1 (BridgeKit) and R2 (Looper/CFRunLoop) as the dominant risks, and G3 remains the next kill point.

## M2 starting checklist

M2 goal (PLAN.md §3), as demos:
- Guest libdispatch runs `dispatch_async` to a global queue, `dispatch_after`, groups and semaphores.
- A guest `CFRunLoop` runs for 2 s with a timer and a socket source.
- A guest `NSFileManager` lists `assets/` through libv zzip from the APK.

Exit: the libdispatch test is stable over 100 repetitions, the struct-layout tests pass, and the printf family matches bionic on 2,000 generated cases.

Week 1: gate fixes.
- [ ] F1: add a CI workflow (`.github/workflows/ci.yml`: build, `cargo test --workspace`, clippy `-D warnings`, `scripts/coverage.sh`) that fails below 100% lines and functions. It must not download an APK.
- [ ] F2: add a hash-gated post-init assert that `OPENSSL_armcap_P == 0` (look up the symbol by name in libApplication and libSecurity).
- [ ] F3: update the README with the status, build prerequisites, `--init-only` usage and the smoke-test command.
- [ ] F4: drop `encoding_hex` from the census CSVs, or record the decision in `docs/m0/opcode-census.md`.
- [ ] F12: settle the crate name against PLAN.md; add a note in `tools/census/README.md` that it is outside the gate.

Scheduler and threads:
- [ ] F7/F11: add a cycle-budget time slice in `bhpc-cpu`/`sched.rs`. On expiry, yield the baton, and give a host watchdog a periodic exit point. Test it with two guest threads, one of which spins on `ldrex`/`strex` waiting for the other.
- [ ] F6: make `pthread_once` a CAS state machine (INIT, IN_PROGRESS, DONE) with waiters through `sched::block`. Test it with init routines that block.
- [ ] `__futex_wait`/`__futex_wake` keyed by guest address, through `sched::block`.
- [ ] Decide whether libdispatch workers stay serialised (baton) for M2. Record the measured throughput of the 100x libdispatch test.

File descriptors and I/O:
- [ ] Virtual fd table for files, pipes, eventfd, sockets and epoll. Implement `poll`/`select`/`pselect`/`epoll_*` on our own objects (`mio` for sockets). The ARM EABI `epoll_event` is 16 bytes and needs a layout test.
- [ ] BSD sockets plus `getaddrinfo` (rebuilt in the guest arena), `inet_*`, `__get_h_errno`.
- [ ] Missing stdio: `fseek`/`ftell` on `funopen` streams (`_seek` callback), `freopen`, and the `sscanf`/`fscanf`/`vsscanf` family. Add `strptime`.

Runtime surface:
- [ ] F9: `dlopen`/`dlsym`/`dlclose`/`dlerror`/`dladdr` in bionic over the loader's module table. `dlopen("libOpenSLES.so")` and `/system/lib/libOpenSLES.so` return NULL. Remove `__isinf`, `__cxa_begin_cleanup` and `__cxa_type_match` from the trap list, implementing them or binding them to guest exports.
- [ ] F8: take `TZ` and `localtime` from the host zone, and add a test that guest `NSLog` time equals host local time.
- [ ] F10: find out which image's `NSBlock` the runtime keeps.
- [ ] F5: add a `--jni-strict` switch that stops on a Java call with method id 0 and on abort-class slots, and attribute `?` calls to the guest caller. The real reflection work stays in M3.

Demos and exit tests (APK runs stay `#[ignore]` behind `BHPC_APK`; coverage comes from NDK fixtures):
- [ ] An NDK fixture (r27, `armv7a-linux-androideabi21`) that runs the same printf cases in the guest, with a generator for 2,000 cases. Compare the output against bionic. Decide whether fixtures are committed or rebuilt in CI (G0 fix 5).
- [ ] libdispatch demo: after `--init-only`, call guest `dispatch_async` to a global queue, `dispatch_after`, `dispatch_group_*` and `dispatch_semaphore_*`. Repeat 100 times with no deadlock.
- [ ] `CFRunLoop` demo: run for 2 s with a `CFRunLoopTimer` and a `CFSocket` source on a loopback socket pair.
- [ ] `NSFileManager` lists `assets/` through libv zzip from the APK mounted read-only.
- [ ] Struct-layout tests for every struct that crosses the boundary in M2 (`stat`, `dirent`, `addrinfo`, `sockaddr*`, `pollfd`, `epoll_event`, `msghdr`/`iovec`, `timeval`/`timespec`).
- [ ] Keep coverage at 100% lines and functions (F1 makes CI enforce it), keep clippy clean, and run the M1 `--init-only` demo on all three APKs as a regression check.
- [ ] Track G3 in M3: if the BridgeKit/DexMaker proxy path cannot be made to work, evaluate the host ObjC-class replacement (+4 weeks).
