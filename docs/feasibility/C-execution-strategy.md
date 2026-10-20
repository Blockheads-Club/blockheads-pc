# Analysis C: Execution strategy for ARM32 game code on x86_64 PCs

Scope: how to run the Blockheads 1.7.4 armeabi-v7a native libraries on a PC with our own loader, with no Android emulator. Other analyses cover the Java/JNI side, graphics and audio. Measurements were taken from the user's own APK, extracted to the scratchpad. Nothing is distributed.

## 1. What the guest code is (measured)

| Fact | Value |
|---|---|
| Libraries | 26 `.so` in `lib/armeabi-v7a/`, 48.5 MB total. `libApplication.so` is 17.2 MB. |
| ELF | ET_DYN, EABI5, **no hard-float flag**, so the ABI is **softfp**: FP args and returns travel in r0-r3 and the stack. |
| ISA | `Tag_CPU_arch: v7`, Thumb-2 in use, ARM ISA also in use. **Mixed ARM and Thumb**: of 5284 exported functions in libApplication, 2334 are Thumb (44%). |
| FP/SIMD | libApplication is **VFPv3 + NEONv1**. Others are VFPv3-D16 (16 double registers, no NEON tag). A JIT must implement VFPv3 and at least the NEON subset the compiler emitted in libApplication, libCoreGraphics and libFoundation. |
| Barriers/atomics | `ldrex/strex` are present, in small numbers. Stripped binaries with no mapping symbols, so raw objdump counts are unreliable. Raw hits for `mrc/mcr` and `svc` are mostly mis-decoded data. Milestone 0 should redo the scan with symbol-bounded decoding. |
| TLS | **No PT_TLS segments** in any lib. Thread-local data uses `pthread_key_*`, so no TLS-register emulation is needed. |
| Relocations | libApplication has 126,763 R_ARM_RELATIVE, 21,887 R_ARM_ABS32, 614 JUMP_SLOT and 77 GLOB_DAT. No TEXTREL and no TLS relocs. A simple relocator is enough. |
| Layout | libApplication: RX 0xE32258 bytes, RW 0x22E6F4 (+bss). 636-byte `.init_array` (159 constructors). `.ARM.exidx` and `.ARM.extab` are used for C++ exceptions. |
| Dependencies | The bundled libs form a closed graph: libv, libSystem (objc runtime plus libdispatch plus malloc), libcxx, libicu, CoreFoundation and so on. |
| Imports from outside the bundle | **764 unique symbols**: 381 GL/EGL (379 from `libgles_apportable`), 320 libc/libm, 49 pthread/sem, 3 `__android_log_*`, 3 `AndroidBitmap_*` (libjnigraphics), `__aeabi_mem*`, `__gnu_Unwind_Find_exidx`, 4 `__cxa_*`, `dlopen/dlsym/dladdr`. Also `__sF`, `_ctype_`, `__stack_chk_guard`, `__errno` and `cacheflush`. |
| JNI | 91 `Java_*` exports in libApplication and 78 in libBridgeKit. The host must provide `JavaVM` and `JNIEnv` (guest-visible function tables) and a small object model. |
| Syscalls | `syscall`, `__futex_wait/_wake`, `epoll_*`, `inotify_*`, `fork/execve` and sockets appear as imports, but libc is **not** bundled. Everything goes through the bionic API. |

Key conclusion: the guest never talks to the kernel directly. It talks to **bionic through a 764-symbol import surface** (about 370 libc/pthread symbols plus GL). That makes HLE of the C library practical, so we do not need a Linux syscall emulator.

## 2. What exists on this machine (nothing installed)

- Host: Ryzen 9 5980HS, 16 threads, 30 GB RAM, Linux 6.18-lts, x86_64, Mesa 26.2.3.
- `qemu-base` and `qemu-system-x86` 11.1.1 are installed. **`qemu-user` and `qemu-user-binfmt` are NOT installed** (`qemu-arm` missing). Both exist in `extra`, version 11.1.1-4.
- No armhf cross toolchain and no `/usr/arm-linux-gnueabihf` sysroot. `clang` (host) exists, and the NDK 27.2 has `armv7a-linux-androideabi*-clang`. That builds bionic-flavoured ARM32 code, not glibc.
- No dynarmic, unicorn, box64 or FEX on the machine, and none in the cargo registry.
- binfmt_misc is mounted but has no ARM entry.

## 3. Option comparison

### Option 1: qemu-arm user mode + armhf sysroot + loader built for ARM

How it works: build our loader as a 32-bit ARM Linux executable. It loads the `.so` files with its own ELF loader and implements the bionic surface by wrapping the **armhf glibc** sysroot. Graphics go through ARM Mesa llvmpipe in the sysroot or a GL forwarding thunk.

- Needs `qemu-user` and a Debian or Arch armhf sysroot. Arch has no official armhf, so this means a Debian multistrap or debootstrap rootfs of about 300 MB or more. The toolchain must be `arm-linux-gnueabihf-gcc`, or clang with `--target=armv7a-linux-gnueabihf`.
- **Bionic ABI is not glibc ABI.** The loader must write shims for: `__sF` and FILE layouts, `_ctype_`, `pthread_mutex_t` (4-byte bionic vs 24-byte glibc), `stat`, `dirent`, `sigaction` and `sigset`, `setjmp` buffers, `__errno`, and the `__stack_chk_guard` variable. Without the shims the game crashes or corrupts memory. With them, the work is the same HLE as in option 2, plus a cross toolchain.
- Threads are real host threads, as in qemu-user (clone). Atomics and futexes are handled by qemu. This is the **most compatible multithreading path** and the cheapest to bring up.
- Performance: QEMU TCG is typically **5-15x slower than native** for integer code. NEON is emulated through vector helpers and is slower. Single-thread TCG on a Zen 3 core at about 4.5 GHz is still well above a 2012 Cortex-A9 at 1 GHz, so playable but not light. Estimate only; it must be measured.
- Graphics: the guest calls GL through the loader's shims. llvmpipe (software) in the ARM guest means **CPU rendering inside emulated code**, which is the worst case. The sane variant is a GL forwarding thunk. Guest GL calls become qemu-user syscalls or hypercalls with no host interface, so you must patch QEMU or use a pipe/shared-memory RPC to a 64-bit host renderer. That is a custom protocol with 380 calls. It is workable, but it is as much work as option 2's direct thunks and slower.
- Distribution: **very heavy.** The user must have qemu-user, the sysroot and the toolchain, or we ship them. QEMU is GPL-2.0, and shipping a sysroot (glibc LGPL) adds more license and size problems. Not user friendly on Windows (QEMU user mode exists only for Linux/BSD). **Windows and macOS are not possible** with this option, because qemu-user does not run there.
- Licensing: running qemu-user as a separate process is aggregation, so it is compatible with GPL-3.0. Linking QEMU code is not (see section 5).
- Verdict: good as a quick **validation vehicle on Linux** (does libApplication init?). It is not a viable product path because of Windows and macOS.

### Option 2: embedded dynamic recompiler in a host x86_64 loader (recommended)

Candidates:

| Engine | License | Guest | Notes |
|---|---|---|---|
| **dynarmic** | **0BSD** | ARMv6K to ARMv8 A32 (ARM and Thumb/T32), VFP, ASIMD (NEON); also A64 | Used by Citra, Yuzu and forks. Host x86_64 and arm64, Windows/Linux/macOS. C++. Upstream `merryhime/dynarmic` now returns 404 (removed), lioncash/dynarmic is archived (2024-03). Maintained forks include `azahar-emu/dynarmic` (0BSD, last update 2026-09-26) and Eden's fork (the Azahar issue says it has improvements), so a fork must be vendored and pinned. |
| unicorn 2 | GPL-2.0 (**GPL-2.0-only; incompatible with GPL-3.0 linking**) | ARM, Thumb, VFP, NEON via QEMU | Easy API (`uc_hook_*`, `uc_mem_*`), but slower per-call hooks, awkward multithreading (one context per thread, global memory), and the license blocks static linking in our GPL-3.0-or-later repo. |
| QEMU TCG as library | GPL-2.0 | full | Not designed for embedding. |
| libriscv/ARMeilleure/Box-like | n/a | n/a | No mature ARM32-on-x86 embeddable JIT besides dynarmic. Writing our own interpreter is a fallback (a Thumb-2 and VFP interpreter is about 10-15 kLOC plus NEON). |

**How imports are trapped.** Put a "thunk page" in guest memory (for example 0xF0000000). Each of the 764 external symbols, plus the `libc.so`-style names, resolves to a unique 4 to 8 byte stub in that page (`bx lr` encoding or an `svc #N`/`udf #N`). The relocator fills GOT entries with those addresses. dynarmic's `ExceptionRaised` / `CallSVC` callback fires when the guest executes the stub, and the host reads r0-r3, sp, d0-d7 for the softfp call, runs the Rust or C++ implementation and writes r0/r1 back. Per-call cost is about 100-300 ns. That is acceptable except for hot calls (`memcpy`, `strlen`, `sinf`, `glDrawArrays`). Hot libc functions can be handled directly in the JIT block via the same callback, because the thunk executes natively in the host with no emulation cost, and the host `memcpy` is faster than emulated code.

**Memory model.** Reserve 4 GiB of virtual address space (`mmap` with `MAP_NORESERVE`, or `VirtualAlloc` on Windows). dynarmic's fast-mem path uses a host page table or the single-base-plus-32-bit-offset scheme, so a guest load is one host `mov`. Guest pointers are 32-bit, and host-owned data (GL buffers, JNI handles) must live in the window or be translated at the thunk boundary. Both are OK. Memory ceiling: the game fits easily (no more than 512 MB).

**Threads.** The game uses pthreads and libdispatch (95 `dispatch_*` symbols are inside libSystem.so, with worker threads built on `pthread_create`). dynarmic has one `Jit` instance per guest thread. The `ExclusiveMonitor` is shared, so `ldrex/strex` work across instances. Our HLE `pthread_create` makes a host thread running a new Jit with its own stack inside the window. Mutex, cond and key calls map to host primitives behind a guest-visible 4-byte handle (bionic `pthread_mutex_t` is 4 bytes, so a table index or a lock-free pointer map works). Futex calls (`__futex_wait/_wake`) map to host futexes or `WaitOnAddress`. The risk is subtle: statically initialised mutexes (all zero or `0x4000`) must keep working with lazy allocation of the host object.

**Callbacks host -> guest.** Needed for JNI entry points (`Java_com_apportable_MainThread_nativeRun`, the touch functions, `GLSurfaceView_nativeInit` and so on), callbacks from `pthread_create`, `qsort` / `bsearch` comparators, `atexit`, signal handlers and ObjC blocks. With dynarmic this is "set r0-r3/pc/sp/lr, set lr to a magic return address, `Run()` until it reaches the magic address". Re-entrancy (guest -> host -> guest -> host) works if each nesting level saves the register state. ObjC blocks and the dispatch function pointers are plain guest calls, as long as the dispatch library is guest code (it is, inside libSystem), so the host never has to know about blocks.

**Floating point and NEON.** The JIT handles VFPv3 with IEEE flush-to-zero choices configurable. Softfp means host thunks must read doubles from register pairs (r0:r1). NEON coverage is the biggest unknown. The A32 ASIMD front end covers most integer, float and load/store forms, and not every instruction (unimplemented ones raise `UnimplementedInstruction`). We must run a symbol-bounded scan of the NEON opcodes the libs actually use and diff it against the fork's decoder table in Milestone 0. Missing instructions can be added upstream-style as IR, or skipped with a fallback interpreter.

**Performance.** dynarmic output is typically **1.5-4x slower than native** on hot loops, much better than TCG. For this game (a 2012-era mobile title) a Ryzen core is much faster than the original phones, so CPU is not expected to be the bottleneck. The expected costs are thunk crossing, GL call translation, and block translation at start-up (a 17 MB lib means a few seconds of first-run JIT, plus cache-clear on `cacheflush`).

**Self-modifying code and `cacheflush`.** Imported, so handle it with `InvalidateCacheRange`. The azahar fork has a recent fix for range invalidation.

**Portability.** dynarmic's x86_64 backend runs on Windows, Linux and macOS (Intel); its arm64 backend runs on Apple Silicon and Windows on ARM/Linux arm64. Option 2 is the only one that covers all platforms in the goal.

Verdict: **recommended.** License-compatible (0BSD), embeddable, multi-thread aware, fastest.

### Option 3: native execution on ARM hosts

- Apple Silicon: **no AArch32 support at all** in hardware (Apple cores dropped it). Not possible natively; dynarmic's arm64 backend still works.
- Windows on ARM: Oryon and Cortex-X class cores lack AArch32 EL0 in most current designs. Windows has no ARM32 userland x-compat on ARM64 for new devices. Not usable.
- Linux ARM64: depends on the SoC. Many modern cores (Cortex-A715 and later, Apple, Oryon) have dropped AArch32. Older Cortex-A53/A72/A76 still run it with `CONFIG_COMPAT=y`. That would let a native armhf loader run the code at full speed on a Raspberry Pi 4 class device. A niche case only.
- Android devices already run the original APK.
- Verdict: only an optimisation for a small set of hosts. The design should allow a "native AArch32" backend behind the same loader interface, but this is not a priority.

## 4. Recommended path: Option 2 (dynarmic fork + HLE loader), Option 1 as a Linux-only oracle

Architecture:

1. **Loader (Rust, `blockheads-pc`)**: reads the APK (zip) at runtime, maps `lib/armeabi-v7a/*.so` into the 4 GiB window with the PT_LOAD layout, applies R_ARM_RELATIVE/ABS32/GLOB_DAT/JUMP_SLOT, resolves symbols across the bundled libs in dependency order, and falls back to thunks for the 764 externals.
2. **dynarmic via a thin C ABI wrapper** (cxx or a hand `extern "C"` shim). Build with CMake under cargo `build.rs`. Vendor a pinned fork commit (0BSD, GPL-3.0-compatible).
3. **HLE bionic** in Rust: libc (about 320 symbols), libm (about 70 are trivial wrappers over the host), pthread (49), stdio (`__sF` FILE table, `funopen`), sockets (map to host sockets), `dlopen/dlsym` (resolve in our module table), `__android_log_*`, `__stack_chk_guard`, `__errno` (per-thread slot in guest memory).
4. **JNI / Java runtime shim**: a hand-written model of the `com.apportable.*` calls the native code makes (analysis A/B), exposed as a guest `JNIEnv` function table.
5. **GL**: thunk the 380 GL/EGL symbols to host OpenGL (GLES 2 / desktop GL via `glow` or `gl`). Apportable's `libgles_apportable` likely emulates GLES1 on GLES2, so in the best case we only need to forward its GLES2 calls. This is a graphics analysis topic.
6. **Audio**: OpenAL imports go to host OpenAL-soft or `cpal`; `libOpenAL.so` is bundled, so only its backend calls (probably to JNI AudioTrack) need replacing.

## 5. Licensing against GPL-3.0-or-later

| Component | License | Compatible? |
|---|---|---|
| dynarmic (azahar/Eden/other forks) | 0BSD (permissive; some downstream forks state GPL-3 with extra grants, so check each commit's SPDX headers and pick a pure 0BSD tree) | Yes. 0BSD code can be combined into a GPL-3.0 work. |
| Its dependencies: fmt (MIT), mcl (MIT), xbyak (BSD-3), oaknut (MIT), Zydis/Catch2/robin-map (MIT) | permissive | Yes |
| Unicorn / QEMU | GPL-2.0 (unicorn: GPLv2 only) | **No for linking** (GPL-2.0-only cannot combine with GPL-3.0-or-later). Fine as a separate process. |
| qemu-user as external tool | GPL-2.0 | Yes as aggregation (Linux validation only, not shipped) |
| Game libs/assets | proprietary | Never distributed. Loaded from the user's APK at runtime. |

## 6. Staged plan, risks and estimates

Effort assumes one experienced developer, working part time on top of the current Rust server project.

| Milestone | Deliverable | Estimate | Main risk |
|---|---|---|---|
| M0 Feasibility scan | Symbol-bounded decode of all libs; list of ARM/Thumb/VFP/NEON mnemonics used; diff against dynarmic's decoder; check `qemu-user` run of a trivial ARM binary on this machine (install is needed, so ask first). | 2-3 days | NEON gaps |
| M1 Load + relocate + run init | Rust loader maps libs, applies relocs, builds thunk table, runs `.init_array` of libv, libSystem, libcxx, libCoreFoundation... through dynarmic, with logging stubs for any unimplemented import. | 2-3 weeks | C++ exception unwinding (`__gnu_Unwind_Find_exidx`, ARM EHABI tables), ObjC runtime `+load` order, `dlopen` self-references |
| M2 libc/pthread/dispatch HLE | Threads, mutexes, TLS keys, stdio, files mapped to a sandbox dir, sockets. libdispatch worker threads run. | 3-4 weeks | Bionic struct layouts, lazy-initialised sync objects, signal usage (`sigaction`, `pthread_kill`) |
| M3 JNI stub + reach `UIApplicationMain` | `JNI_OnLoad`, fake `JNIEnv`, calling `nativeRun` -> `main` -> `UIApplicationMain`; AssetManager served from the APK. | 3-4 weeks | Size of the Java surface (169 native entry points, many Java callbacks) |
| M4 First GL frame | EGL context on host window (winit + glutin), GL thunks, CoreGraphics/Pango text rendering running in guest code. | 3-5 weeks | GLES1 emulation layer behaviour, shader compatibility (GLSL ES to desktop GL) |
| M5 Main menu, input | Touch from mouse, keyboard mapping, timers, run loop (`RunLoop_nativeNext`), audio via OpenAL backend. | 3-4 weeks | Run loop timing, audio latency |
| M6 Gameplay | Offline world loads and plays at a stable 30-60 fps; save files; IAP/ads/news stubs removed or faked. | 4-6 weeks | NEON bugs, performance of CoreGraphics software paths in the JIT |
| M7 Multiplayer to our Rust server | CFNetwork/socket HLE over host sockets, TLS via host or `libSecurity` stubs, connection to the Rust 1.7.1 server. | 2-3 weeks (the server work already exists) | Protocol version differences (client is 1.7.4 to 1.7.6, server implements 1.7.1) |
| Polish, packaging | Windows/Linux/macOS builds, CI | 3-4 weeks | Code signing, JIT permissions on macOS (`MAP_JIT`) |

Total: **about 6-9 months part time (25-35 developer-weeks)** to M6, including M7 about 7-10 months. M1 alone is a decision point: if init runs after 3 weeks the project is likely viable; if the ObjC runtime or EHABI unwinding hits a wall the cost estimate should be doubled.

Risks, in order:
1. The Java/JNI surface (169 entry points plus calls back into Java) is bigger than the CPU side. It is the main effort driver (other analyses).
2. The dynarmic fork status (upstream removed). Mitigation: vendor and pin a fork.
3. NEON gaps (checked in M0).
4. Stripped libraries and Apportable's custom ObjC runtime make debugging hard. Build a guest-side tracer (log every thunk call with the caller address) in M1.
5. macOS needs a JIT entitlement and `MAP_JIT`. Windows needs RWX or W^X toggling.
6. Legal/product: users must provide the APK. Fine under the stated policy. Anything that bundles Apportable or game bytes is forbidden.

## 7. Short answer

- Instruction set: ARMv7, ARM plus Thumb-2, softfp, VFPv3, NEONv1 (libApplication only).
- Import surface: 764 symbols (381 GL/EGL, about 370 libc/pthread/libm, JNI via 169 native exports).
- Best path: dynarmic-based embedded JIT with HLE bionic, 0BSD, runs on all three desktop OSes.
- qemu-user is not installed here, there is no armhf sysroot, and QEMU-based paths are Linux-only and GPL-2.0.
- Estimate: 6-9 months part time to gameplay.
