# bhpc-cpu

Embedded ARM32 JIT (vendored dynarmic, A32 frontend) behind a small C ABI shim, plus the Rust
`Cpu` wrapper. Guest memory comes from `bhpc-mem`. GPL-3.0-only (dynarmic itself is 0BSD).

## Host platform support

| Host | Status |
|---|---|
| Linux x86_64 | Built, tested, 100% line/function coverage in CI. |
| Windows x86_64 (MSVC) | Written, **never built or run** (no Windows machine available). |
| macOS x86_64 | Written, **never built or run**. |
| macOS arm64 (Apple Silicon) | Written, **never built or run**. |

`.github/workflows/ci-crossplat.yml` builds and tests the host-facing crates on `windows-latest`,
`macos-13` and `macos-14`. Until it has gone green, everything below the table is unverified.

### What is verified here (Linux)

- `cargo check`/`clippy` of `bhpc-mem` for `x86_64-pc-windows-gnu`, `x86_64-pc-windows-msvc` and
  `aarch64-apple-darwin` compile the Windows and POSIX/macOS code paths of the memory window.
- The per-OS build decisions (archive extension, link order, C++ runtime, CMake definitions) live in
  `src/buildcfg.rs`, are shared with `build.rs` and are unit-tested on Linux for every target triple.
- `bhpc-cpu` itself cannot be cross-checked: its `build.rs` runs CMake with a C++ compiler for the
  *target*, and no Windows/macOS C++ cross toolchain exists here.

### Design and unverified assumptions

**Guest window (`bhpc-mem`)**: `sys_unix.rs` (mmap `PROT_NONE`, `mprotect`, `munmap`) and
`sys_windows.rs` (`VirtualAlloc` `MEM_RESERVE`/`PAGE_NOACCESS`, `MEM_COMMIT`, `VirtualFree`) sit behind
`cfg(unix)`/`cfg(windows)`. Failure injection and the shared tests are OS-neutral.
- macOS has no `MAP_NORESERVE`; the flag is only used on Linux/Android. Not verified that a 4 GiB
  `PROT_NONE` reservation is accepted without it (it should be: untouched PROT_NONE pages are free).
- Apple Silicon has 16 KiB host pages while guest pages are 4 KiB. `mprotect` ranges are widened to
  whole host pages, so a guest access to an *unmapped* 4 KiB page that shares a host page with a mapped
  one will not fault. Only the software `is_mapped` check (and fastmem-off callbacks) see it.
- Windows commit charge: every `map` commits its pages, so a big guest mapping counts against the
  system commit limit. The 4 GiB reserve itself is free.
- Windows error codes are `GetLastError` values (`ERR_NOMEM = ERROR_NOT_ENOUGH_MEMORY`); only
  displayed, never matched.

**JIT code memory** is allocated by dynarmic, not by us. x86_64 backend: `VirtualAlloc` on Windows,
`MAP_JIT` on macOS (`block_of_code.cpp`). arm64 backend (Apple Silicon): oaknut's `CodeBlock` uses
`MAP_JIT` plus `pthread_jit_write_protect_np`, toggled per thread inside dynarmic. Consequences, all
unverified:
- A macOS binary that JITs normally needs the `com.apple.security.cs.allow-jit` entitlement when
  hardened/signed. Unsigned `cargo test` binaries run without it locally; a shipped app needs it.
- `pthread_jit_write_protect_np` is per thread: a `Cpu` must be driven from the thread that runs it
  (already the contract: `Cpu` is `Send` but only used by one thread at a time).
- dynarmic's arm64 backend for A32 guests is less exercised upstream than x86_64; the test-suite will
  tell. The first macOS-14 CI failure is most likely here (or in the Boost step).

**Fastmem on macOS**: dynarmic's Mach exception handler runs `mig` and writes generated sources into
the vendored (verbatim) tree. `cmake/CMakeLists.txt` pre-sets `MACH_EXC_DEFS_DIR` to NOTFOUND so the
generic (no-op) handler is linked, and `CpuOptions::default()` turns fastmem off on macOS so faults go
to the callbacks instead of crashing. Fastmem on macOS is therefore slower and untested; enabling it
would need the Mach handler built out of tree (a vendored-code change).

**Windows fastmem** relies on dynarmic's `exception_handler_windows.cpp` (vectored exception handler).
Fault-on-`PAGE_NOACCESS` for reserved-but-uncommitted pages is the same mechanism as POSIX
`PROT_NONE`; unverified.

### Build requirements

CMake >= 3.16, a C++20 compiler (MSVC 2022 / Apple clang / gcc / clang) and Boost headers >= 1.57.
- Windows: Visual Studio 2022 build tools. Boost: point `BHPC_BOOST_ROOT` at a Boost tree (the CI
  uses the one preinstalled on runners). The build forces `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL`
  because Rust links the dynamic CRT even in debug.
- macOS: `brew install boost cmake`; `export BHPC_BOOST_ROOT=$(brew --prefix boost)`. Deployment
  target is 11.0 on arm64 and 10.15 on x86_64. Do not set `CMAKE_OSX_ARCHITECTURES` (dynarmic then
  treats the build as multi-arch and skips host detection).
- `BHPC_BOOST_ROOT` is optional everywhere; without it CMake's normal Boost search applies. The
  wrapper sets policy CMP0167 to OLD so `FindBoost` keeps working on CMake >= 3.30.
- MSVC archives are `*.lib` (no `lib` prefix); `build.rs` collects both `.a` and `.lib` and links the
  C++ runtime explicitly only where the toolchain does not (libstdc++ / libc++).
