# G0 gate decision

**Date:** 2026-10-17
**Reviewer:** G0 gate reviewer (workflow agent)
**Inputs:** M0 JIT spike (commits `84d1430`, `cbf1284`) and opcode census (commit `6bc2c2d`), all on `dev`, not pushed.

## Verdict: PASS-WITH-FIXES (proceed to M1)

The G0 rule in `docs/feasibility/PLAN.md` §3 says to kill the project, or switch to an interpreter, if NEON or Thumb gaps exceed 1-2 weeks of work. The measured gap is close to zero:

- **Thumb:** the 25 game libs contain no Thumb code. That is 0 T16/T32 instructions and 0 odd-addressed `FUNC` symbols.
- **NEON:** every live NEON and VFP instruction has a dynarmic decoder.
- **SHA-1:** the only instructions dynarmic cannot decode are the ARMv8 SHA-1 ops and 8 iWMMXt `ldc2/stc2` words. Static evidence says both sets are unreachable on our CPU model.

All three M0 exit criteria are met. The listed fixes are housekeeping plus one design decision. Together they take about 1 week, and none of them blocks the JIT approach.

## M0 exit criteria, re-verified

| # | Criterion | Evidence (re-run by reviewer) | Result |
|---|---|---|---|
| 1 | dynarmic fork builds from `cargo build` on Linux | `cargo build --release` is clean and incremental. The vendored fork is azahar-emu/dynarmic `a46601580d`, licensed 0BSD, and its pins are in `third_party/dynarmic/VENDORED.md`. `cargo test --workspace --release` gives 85 passed, 0 failed (29+20+15+16+5). | **Met** |
| 1b | Demo: `bhpc-run fixtures/hello.so` | It prints `hello from ARM` and 13/13 checks `[ok]`, covering relocations, ARM/Thumb interworking, NEON, VFP softfp, thunk args and C++ throw/catch through `dl_unwind_find_exidx`. Summary line: `1669 relocations, 41 thunks`. Exit code is 0. | **Met** |
| 2 | Symbol-bounded opcode census diffed against dynarmic: 0 unsupported, or a fix plan under 1 week | See `opcode-census.md`. The reviewer spot checks are listed below. | **Met.** There are 0 live unsupported opcodes, and the dead ones have a guard plan under 1 day. |
| 3 | Thunk ABI under 200 ns per call | `bhpc-run --bench-thunk 20000000` gives **11.2 ns/call** (5.9 ns over a guest `bx lr`). The spike reported 12.1-12.3 ns. | **Met** (about 18x margin) |

### Census spot checks (1.7.6, libs extracted in scratchpad `pcport/x176`)

- **Thumb absent.** `llvm-readelf --dyn-syms` on all 25 libs finds 0 defined `FUNC` symbols with an odd address. This agrees with T16=T32=0.
- **libApplication attributes.** `Flags 0x5000000` (softfp), `FP_arch` and `Advanced_SIMD_arch` are present.
- **SHA-1 counts.** `llvm-objdump --mattr=+v8,+crypto,+neon` finds the same counts in both libApplication and libSecurity: sha1c 6, sha1p 10, sha1m 5, sha1su0 16, sha1h 20, sha1su1 16.
  - The NV-coded group (sha1c/p/m/su0) totals 37 per lib, so **74** across both libs. The census CSV has 74 rows for 1.7.6.
  - The `arm_UDF` group (sha1h/su1) totals 36 per lib, so **72**. Both totals match the census.
- **SHA-1 is dead.** The census left the `OPENSSL_armcap_P` initializer unverified; it is now closed:
  - `OPENSSL_cpuid_setup` (0x1c4738) is a bare `bx lr`.
  - `sha1_block_data_order` reads the capability word PC-relative at **0x10607cc**, which is in `.bss` (0x1060490 plus 0x5ee9).
  - No dynamic relocation targets that address.
  - So the word is 0, and the SHA-1 and NEON OpenSSL paths are never taken. The integer path runs instead.
- **Relocation types across all 25 libs** (an input for M1):
  - 225,375 `R_ARM_RELATIVE`, 31,541 `ABS32`, 4,854 `JUMP_SLOT` and 516 `GLOB_DAT`. Nothing else appears.
  - No `TEXTREL`. All 25 libs are `DF_SYMBOLIC`.
  - The spike loader's supported set is therefore enough. The total is about 262k relocations, not the 150k in PLAN.md.

## Required fixes (all small, done during the first week of M1)

1. **Coverage gate (AGENTS.md rule).** Line coverage is 98.29% and functions are 96.30%.
   - Cover or justify the gaps: the `bhpc-mem` mmap/mprotect error paths (use an injectable OS layer or `#[cfg(coverage)]`-gated failure hooks), the `bhpc-cpu` interpreter-fallback trampoline, and the `main()` and bench error branches.
   - Add a coverage config that excludes only `third_party/`, the C++ shim and the test-ELF builder.
   - Add a CI workflow that fails below 100%. The repo has no CI yet.
2. **Dead-instruction guard.** Map dynarmic `UnpredictableInstruction`/`UndefinedInstruction` exceptions to a stop reason that names the guest PC and symbol. Add a load-time assert, gated on the library hash, that `OPENSSL_armcap_P` is still 0 after `.init_array` runs. Effort: under 1 day.
3. **Nested host-to-guest calls: decision record before M1 init work.**
   - `qsort` is imported by 10 libs (CoreFoundation, icu, System and others) and `bsearch` by 4. `__cxa_atexit` is in all 25. `funopen` is in CoreGraphics and libv.
   - Constructors can reach `qsort`/`bsearch`, so this matters already in M1.
   - Options: (a) a re-entrant call stack using a child `Jit` that shares memory and the exclusive monitor, or (b) continuation-style thunks.
   - Recommendation: (a). M2 also needs one Jit per thread and a shared monitor, so this is the same work.
4. **Build prerequisites in the README.** List Boost headers, CMake >= 3.16 and a C++20 compiler. Note that every cargo profile rebuilds dynarmic (about 3.5 min) and that only x86_64 Linux is verified.
5. **Fixture policy.** Decide whether `fixtures/hello.so` (165 KB, our own code) stays committed, or whether CI builds it with the NDK. The recommendation is to keep it committed, plus a CI job that rebuilds it and diffs the result.
6. **Census data hygiene.** `opcode-census-unsupported.csv` and `-odd.csv` contain raw 32-bit instruction words from the game libs, alongside the addresses.
   - This matches the patcher's "address plus expected word" model, so it is probably acceptable.
   - Confirm it against the PLAN.md §5 no-game-bytes rule, or drop the word column.
7. **Housekeeping.**
   - Rename `bhpc-loader` to `bhpc-elf` or update PLAN.md, so the two match.
   - Decide whether `tools/census` (Python) is a dev tool outside the coverage gate. The recommendation is yes: document it in `tools/census/README.md`.

Not required for G0, but tracked:
- the thunk table is per-Cpu (it must be shared for threads; M2);
- host SIGSEGV handler interplay and guest stack overflow;
- arm64, Windows and macOS builds (M8);
- re-measuring the thunk cost with real thunks (`memcpy`, `puts`, GL).

## M1 starting checklist

M1 goal (PLAN.md §3): `bhpc --apk <path> --init-only` maps all 25 libs, relocates them, runs every `.init_array`, prints a thunk histogram, and passes the ObjC/CF/NSLog smoke test.

- [ ] Fixes 1-4 above are landed (coverage CI, undefined-instruction stop reason, nested-call design plus implementation, README prerequisites).
- [ ] APK reader: open the user APK read-only (zip crate), locate `lib/armeabi-v7a/*.so`, and check SHA-256 against a known-hash table for 1.7.4, 1.7.5 and 1.7.6. Nothing is extracted to disk.
- [ ] Multi-module loader:
  - Load in DT_NEEDED topological order and lay out all 25 libs in the 4 GiB window.
  - Use per-segment protections and handle `PT_ARM_EXIDX` for each module.
  - Map modules by address for `dl_unwind_find_exidx`/`__gnu_Unwind_Find_exidx` and `dladdr`.
- [ ] Symbol resolution: all 25 libs are `DF_SYMBOLIC`, so search self, then global load order, then host thunks. Unresolved imports bind to the trap thunk, are counted, and are reported.
- [ ] Relocations: about 262k total, only RELATIVE, ABS32, GLOB_DAT and JUMP_SLOT. Add a test that the counts match the census per lib.
- [ ] `.init_array`/`DT_INIT` runner in dependency order (159 entries in libApplication), with per-constructor watchdog and thunk trace.
- [ ] `bhpc-bionic` skeleton:
  - Move the fixture HLE out of `bhpc-run`.
  - Cover the libc subset that constructors hit: malloc family, str/mem, `__cxa_atexit` (record only), `pthread_once`/key/mutex, `__errno`, `__stack_chk_guard`, `__sF`, `__system_property_get`, `__android_log_*`, `qsort`/`bsearch` through nested calls.
  - Add struct-layout unit tests from day one.
- [ ] Thunk-call histogram output and an "unimplemented imports hit during init" counter. The M1 exit criterion requires it to be 0.
- [ ] Host-driven smoke test, `#[ignore]` and behind `BHPC_APK`: `objc_getClass("UIApplication")`, `CFStringCreateWithCString`, and `NSLog(@"…")` reaching `tracing`.
- [ ] Guest backtrace on stop: `.ARM.exidx` walk plus dynsym symbolisation (R7). Start this early, because stripped-lib crashes are the main M1 time sink.
- [ ] Track G1: if M1 slips more than 2 weeks past its 3-week estimate because of the ObjC runtime, EHABI or jemalloc, re-estimate at ×2 or stop.
