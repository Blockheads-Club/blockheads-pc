# blockheads-pc: run the user's own Android Blockheads build natively on PC

Status: decision document. Inputs: [A: native imports](A-native-imports.md), [B: JNI/Java surface](B-jni-java-surface.md),
[C: execution strategy](C-execution-strategy.md). The lead engineer spot-checked the claims below against the 1.7.4 binaries (section 9).

## 0. TL;DR

| Question | Answer |
|---|---|
| Recommendation | **GO, staged.** Commit only to **M0 and M1 (about 4.5 dev-weeks)**, then decide again at gate G1. There are three kill gates (G0, G1, G3). |
| Approach | So-loader port: a Rust host loads the 25 ARM32 `.so` files from the user's APK, runs them in an **embedded ARM JIT (dynarmic, pinned 0BSD fork)**, and replaces Android with high-level emulation (HLE) of bionic, a fake JVM, GLES-to-host-GL, OpenAL backed by SDL audio, and SDL windowing and input. |
| Why it is tractable | The guest never makes syscalls. It needs only **764 imported symbols**: about 304 libc/bionic, 68 libm, 53 pthread, 380 GL plus 1 EGL, 3 liblog, 3 jnigraphics and 5 libdl. The ObjC runtime, libdispatch, CoreFoundation, Foundation, UIKit, Skia, FreeType, Pango, ICU, zlib, OpenAL-soft, ENet and jemalloc are **all bundled guest code**, so we run them rather than rewrite them. Assets are read by libv's bundled zzip directly from the APK. |
| Biggest cost | The **fake Java world**. That is about 28 entry points and 130-180 Java members to reach the menu, and about 90 classes and 350-450 members for full gameplay. These come from libBridgeKit's reflection bridge plus a Looper/MessageQueue that is merged with CFRunLoop. |
| Effort | **About 31 dev-weeks** (range 26-42) for gameplay plus multiplayer on Linux, Windows and macOS. The first visible frame (M4) comes after about 15 dev-weeks. |
| Platforms | Linux x86_64 first, then Windows x86_64, then macOS (Intel and Apple Silicon, using dynarmic's arm64 backend). |
| Legal | The user supplies the APK at runtime and it is read in place. We never ship, cache or commit game bytes. All test fixtures are our own C code built with the NDK. The project is GPL-3.0-or-later and every dependency is compatible with that. |

## 1. Chosen architecture

```
 +--------------------------- host process (Rust, x86_64 or arm64) ----------------------------+
 |  bhpc-app: SDL2 window, event pump, input mapping, config, APK picker                      |
 |      |                                                                                      |
 |  bhpc-jni: fake JavaVM/JNIEnv (233-slot tables in guest memory) + Rust "android.*" and     |
 |            "com.apportable.*" classes, Looper/MessageQueue/Handler, GLThread emulation      |
 |  bhpc-bionic: HLE libc/libm/pthread/stdio/VFS/virtual-fd/epoll/sockets/dl/log              |
 |  bhpc-gles: 380 GL thunks (code-generated from Khronos gl.xml) -> host GLES2 (Mesa/ANGLE)  |
 |  bhpc-audio: fake AudioTrack + MediaPlayer(AAC via symphonia) -> SDL audio                 |
 |      ^  thunk calls (SVC #n)                                                               |
 |  bhpc-cpu: dynarmic A32 JIT, 1 Jit per guest thread, shared ExclusiveMonitor               |
 |  bhpc-mem: 4 GiB reserved guest window (fastmem), guest heap for host-owned objects        |
 |  bhpc-elf: ELF32 ARM loader, R_ARM_{RELATIVE,ABS32,GLOB_DAT,JUMP_SLOT}, symbol resolution  |
 |      |                                                                                      |
 |  [guest] 25 .so from user's APK: libv(jemalloc,zzip,unwind) libcxx libSystem(objc,dispatch)|
 |          CF/Foundation/CFNetwork/ICU/xml2/CoreGraphics/Pango/CoreText/BridgeKit/OpenAL/     |
 |          libgles_apportable/libApplication (game + UIKit)                                  |
 +---------------------------------------------------------------------------------------------+
```

### 1.1 Language and process model
- The host is **Rust**, a cargo workspace in `blockheads-pc/` with these crates: `bhpc-elf`, `bhpc-mem`, `bhpc-cpu`, `bhpc-bionic`, `bhpc-jni`, `bhpc-gles`, `bhpc-audio`, `bhpc-app`, plus `fixtures/` (C sources).
- Everything runs in one process with no IPC. Guest threads are real host threads, each running its own dynarmic `Jit`.
- The host thread layout mirrors Android:
  - **host main**: the SDL event pump.
  - **"MainThread"**: big stack; runs `nativeRun`, then `UIApplicationMain`, then the unified run loop.
  - **"GLThread"**: emulates `GLSurfaceView`; its loop is `drawFrame` then swap.
  - libdispatch workers, created by guest `pthread_create`.

### 1.2 CPU: dynarmic (chosen), with an interpreter as the fallback

| Choice | Decision |
|---|---|
| Engine | **dynarmic A32**, vendored as a pinned commit of a maintained fork (azahar-emu or Eden). The license is 0BSD; we verify the SPDX header of every vendored file. The dependencies (fmt, mcl, xbyak, oaknut, robin-map) are MIT/BSD. |
| Binding | A C++ shim of about 300 LOC with an `extern "C"` API, built by the `cmake` crate in `build.rs`. A Rust trait `Cpu` hides it, which leaves room for an interpreter backend or a native-AArch32 backend later. |
| Guest features needed | ARMv7-A ARM and Thumb-2 interworking (44% of libApplication's exports are Thumb), VFPv3, and NEONv1 in libApplication. softfp ABI (`e_flags=0x5000000`, no hard-float flag), so FP values pass in core registers. |
| Memory | Reserve 4 GiB with no backing commit and map PT_LOAD segments at a chosen base. Guest pointer = `base + u32`. dynarmic fastmem makes each guest load one host `mov`. Host-owned data the guest must see (GL strings, JNI tables, FILE structs, `_ctype_`, thread stacks) is allocated from a host-managed guest arena. |
| Import trap | A thunk page at `0xFFFF0000-` holds one `svc #n` / `bx lr` pair per import. GOT and PLT entries point there. The dynarmic `CallSVC` callback dispatches to `HOST_FNS[n]` **without leaving JIT `Run()`**, at an estimated 50-150 ns per call. |
| Host-to-guest calls | `call_guest(addr, args)` sets r0-r3 and pushes the rest, sets `lr` to a magic return stub, and runs until it hits that stub. This path serves JNI natives, `pthread_create` start routines, `qsort` comparators, `atexit` and `__cxa_atexit`. It is re-entrant: the register state is saved per nesting level. |
| Thunk ABI in Rust | A `#[guest_fn] fn strlen(s: GPtr<u8>) -> u32` proc-macro generates the softfp marshalling (u64 and f64 values in aligned register pairs, stack spill, varargs readers for the printf family, `open`, `fcntl`, `ioctl` and `syscall`). |
| Self-modifying code | Map `cacheflush` to `InvalidateCacheRange`. |
| Fallback | If dynarmic has fatal NEON gaps, add the missing IR emitters. If that also fails, write our own Thumb-2/VFP/NEON interpreter (10-15 kLOC). That is slow but correct, and it is the G0 kill criterion. |

**Rejected options.**
- qemu-user with an armhf sysroot: Linux only, needs GPL-2.0 tooling, and we would still have to write every bionic shim.
- Unicorn: GPL-2.0-only cannot link into a GPL-3.0-or-later project, and it is slow per hook.
- Native AArch32: Apple, Oryon and newer Cortex cores do not have it.

### 1.3 bionic HLE (bhpc-bionic)
About 304 libc symbols, 68 libm, 53 pthread, 5 dl and 3 log. The full list is in `A-native-imports-symbols.txt`. We implement these in Rust over `std` plus `libc` and `windows-sys`.

| Area | Design |
|---|---|
| **Struct marshalling** | We keep a table of every bionic ARM32 layout that crosses the boundary: `stat` (bionic arm32 `struct stat`, 104 B), `dirent` (u64 ino, i64 off, u16 reclen, u8 type, name[256]), `tm`, `timeval`/`timespec` (32-bit `time_t`), `addrinfo`/`hostent` (32-bit pointers, rebuilt in the guest arena), `sigaction`, `rlimit`, `rusage`, `utsname`, `statfs`, `pollfd`, `epoll_event` (16 B on ARM EABI, unpacked with a u64 aligned at offset 8; only x86_64 packs it to 12 B, so it must not be copied straight through to a host epoll), `msghdr`/`iovec`. Each struct is a `#[repr(C)]` Rust struct with layout unit tests. |
| **errno** | `__errno()` returns a per-thread `int*` that lives in guest memory. |
| **stdio** | `__sF[3]` plus a pool of bionic-layout `FILE` structs (84 B) in guest memory. We always keep `_r = _w = 0`, so the guest's inline `getc`/`putc` macros fall into `__srget`/`__swbuf`/`__srefill`, which we own. The real buffering stays on the host side. `funopen` and `fgetln` are implemented with guest callbacks. libv wraps `fopen`/`open`, so most game file I/O already arrives as plain `open`/`read`. |
| **printf family** | A Rust formatter that reads guest `va_list` (a pointer into the guest stack) for `vsnprintf`/`vfprintf`/`vsscanf` and the like. These are the riskiest about 400 LOC and get exhaustive differential tests against NDK-built fixtures. |
| **pthread** | Guest `pthread_mutex_t` and `pthread_cond_t` are 4 bytes. We use a side table keyed by guest address, created lazily, and honour the static initialiser values `0` and `0x4000` (recursive). `pthread_once` uses an atomic CAS on the guest word. Keys are a host-side TLS map. `__futex_wait/_wake` use the `parking_lot_core` park/unpark keyed by guest address, which is portable (Windows too). `pthread_create` allocates the guest stack and guard, creates the host thread and a new Jit, and calls the start routine. |
| **File system (VFS)** | Every guest path goes through a mount table: `/data/app/<pkg>/base.apk` (= `PACKAGE_CODE_PATH`) maps to the user's APK, read-only. `HOME`, `TMPDIR`, `CACHEDIR` and `ANDROID_DATA_DIRECTORY` map to the per-OS data directory (section 1.8). `/proc/cpuinfo`, `/proc/meminfo`, `/proc/self/*` and `/sys/devices/system/cpu/*` are **synthesised**: 4 cores, `neon vfpv3` features, and an ARMv7 `CPU part` consistent with the JIT. `/system/etc/hosts` and `/etc/resolv.conf` come from host equivalents. `/dev/urandom` is backed by `getrandom`. Everything else returns ENOENT and is logged. |
| **Virtual fd table** | Guest fds are our own handles: files, sockets, pipes, eventfd and epoll. `epoll_*`, `poll`, `select` and `pselect` (libdispatch and CFRunLoop use them) are implemented **on our own objects**, with sockets via `mio`. This is what makes Windows work: there is no host epoll and no POSIX pipe there. `inotify_*` returns a dummy fd that never fires. |
| **Sockets** | BSD API mapped to `socket2`/`mio`. Address structs are binary compatible (Linux layout). `getaddrinfo` uses the host resolver and rebuilds the result list in guest memory. |
| **dl** | `dlopen` resolves against the loader's module table. `"libOpenSLES.so"` **returns NULL** on purpose (section 1.6). `dlsym` covers the guest exports and our host tables. `dladdr` returns guest module info. |
| **Exceptions, setjmp, unwinding** | `__gnu_Unwind_Find_exidx(pc)` returns the module's `PT_ARM_EXIDX`, so C++ and ObjC unwinding run fully in guest code. `setjmp`/`sigsetjmp` and `longjmp` are host thunks that save and restore r4-r11, sp, lr and d8-d15 of the *calling Jit* into the guest `jmp_buf`. This requires the thunk ABI to have mutable access to guest registers. |
| **Signals** | `sigaction` is recorded only. `raise`/`abort` produce a host panic with a guest backtrace. `pthread_kill(t, 0)` reports liveness. `fork`/`exec*` return ENOSYS. |
| **Misc** | `__stack_chk_guard` is random. `__page_size` is 4096. `_ctype_` and `_tolower_tab_` are bionic tables copied into guest memory. `__system_property_get` returns a small property map (`ro.build.version.sdk=19`, model and so on). `__android_log_*` goes to `tracing`. |

### 1.4 Fake JVM (bhpc-jni)
- **Tables.** A `JavaVM` table of 8 slots and a `JNIEnv` table of 233 slots live in guest memory and point at thunks. About 60-70 slots are real; the rest log and abort, so missing behaviour is loud rather than silent.
- **Object model.**
  - `jobject` values are 32-bit handles (index plus generation) into a host heap of `JObject { class: ClassId, data: enum {...} }`. Local and global refs are tracked per JNI frame.
  - Strings are stored as Rust `String`, and arrays as `Vec<T>`. Pinned arrays (`GetByteArrayElements` and the like) are copied into the guest arena, then copied back on release.
- **Class registry.**
  - Each class has a `(name, sig) -> HostMethod` map. `FindClass` and `GetMethodID` **never fail**: unknown members get a dummy ID and are logged. A call to an unimplemented member logs the class, name, signature and guest caller, then returns zero or null.
  - The trace log is the work queue for M3 to M6. B §5.3 says signatures arrive as plain C strings at runtime, so the trace is complete.
- **DexMaker and proxies.** BridgeKit generates listener proxies through `com.google.dexmaker`. We fake the 5 DexMaker classes so that `generateAndLoad` returns a host "proxy class". Interface calls on that class go to `ReflectionProxy.invoke`, the guest native, with an `Object[]` of arguments. If that turns out heavier than expected, the fallback is to patch the 5 BridgeKit call sites.
- **Looper and run loop.**
  - We provide a host `MessageQueue` with timed messages, idle handlers, cross-thread `sendMessage`/`post(Delayed)`, and `hasMessages`/`removeMessages`.
  - `RunLoop.nativeNext` is *guest* code. It calls back into our queue (a blocking pop with a timeout), and libdispatch posts main-queue work through it. The pop wakes on new messages and on SDL-driven input injection.
- **Boot sequence, done by the host instead of Java.**
  1. Call `JNI_OnLoad` of libv, libBridgeKit, libApplication and libOpenAL.
  2. Call `LibraryLoader.setenv` for every variable listed in B §2.3. Do **not** call `Library.resolveLinkages`, because our loader already linked everything.
  3. Call `VerdeActivity.nativeOnCreate`, `nativeOnStart` and `nativeOnResume`.
  4. Post `MainThread.nativeRun` to the MainThread queue.
  5. Once the GL context exists, call `GLSurfaceView.nativeInit`, `GLSurfaceRenderer.surfaceCreated/surfaceChanged`, then `Lifecycle.contextDidBecomeValid`, `applicationWillEnterForeground` and `applicationDidBecomeActive`.
  6. Call `nativeRootGeometryChanged`.
- **Stub classes** with canned answers:
  - IAP: `isBillingSupported=false`, then fire `setupFinished`.
  - Flurry and Heyzap: no-ops, `isAvailable=false`.
  - PlusGames: signed out. NoodleNews: nothing pending.
  - Reachability: Wi-Fi. MotionManager: not available. Notifications: no-op.
  - `BlockheadsWebView.nativeSetWelcomeMessage` path: we render it as a simple overlay or dismiss it. This removes the Android welcome-popup deadlock class of bug by construction.
- **Implemented classes, roughly 90 by gameplay (see B §5.2):**
  - `Build`, `DisplayMetrics`, `SharedPreferences` (persisted as JSON), `Context.get*Dir`, `Settings.Secure.ANDROID_ID` (a stable random ID per install).
  - `Locale`, `TimeZone`, `UUID`, the collections, `Toast` (an overlay), `Intent` (open URL in the host browser), clipboard.
  - `HttpClient` family over `ureq` with rustls.
  - `AudioTrack` and `MediaPlayer`.
  - `ui.Window`, `ui.View` and `ui.Device` as accept-and-store objects.
  - `ui.TextField`/`TextView` plus `KeyHandler` for text entry.
  - `ui.AlertView` as a modal overlay.

### 1.5 Graphics (bhpc-gles) and windowing
- **Window.** **SDL2** through the `sdl2` crate with the `bundled` and `static-link` features. SDL is zlib-licensed and gives one dependency for window, GLES context, audio, text input/IME, gamepads and HiDPI. winit plus glutin plus cpal would mean three dependencies and weaker IME. SDL3 can replace SDL2 later behind `bhpc-app`.
- **GL API.** We request a **GLES 2.0 context**: `SDL_GL_CONTEXT_PROFILE_ES`, either Mesa EGL on Linux or **ANGLE** (BSD-3) on Windows (D3D11) and macOS (Metal). The game's 46 vertex and 46 fragment shaders are GLSL ES with precision qualifiers (checked: `uniform highp vec4 light;`), and the manifest sets `apportable.opengles2`. ES end to end therefore means **no shader translation**.
- **Thunks.** We generate all 380 entry points from `gl.xml` at build time.
  - Pointer arguments are translated as guest address plus base. Client-side vertex arrays work because the guest window *is* host memory.
  - `glGetString` and `glGetStringi` copy into a guest string pool.
  - `glMapBufferOES` returns a guest-arena shadow that is flushed on unmap.
  - Vendor extensions (QCOM, AMD, NV, IMG, APPLE) return "unsupported": the extension string is filtered and the entry is a NULL-safe stub.
- **ES1 symbols.** libApplication imports `glOrthof`, `glFrustumf` and the rest of the ES1 set directly. If the trace shows them called on the ES2 path, ANGLE's GLES1 front end or a small matrix-stack shim covers them. That is a risk (R6), not a plan item.
- **EGL.** `eglGetCurrentContext` is the only native EGL import. The Java-side `VerdeActivity`/`GLSurfaceView` methods map onto SDL as follows:
  - `swapBuffers`, `makeCurrentContext`, `createSharedContext` and `destroyContext` use `SDL_GL_SwapWindow`, `SDL_GL_MakeCurrent` and `SDL_GL_CreateContext` with `SHARE_WITH_CURRENT_CONTEXT`.
  - `getScreenWidth/Height` and `setContentSize` map to the window size divided by the content scale.
- **GLThread emulation.** One host thread owns the primary context and loops `drawFrame(J,GL10)`, then `SDL_GL_SwapWindow` with vsync. `GLSurfaceDispatch.AndroidGLSurfaceBindThread` is honoured for shared contexts on other threads.

### 1.6 Audio (bhpc-audio)
- libOpenAL (OpenAL-soft, bundled) probes its backends in the order `opensles`, `audiotrack`, `null` (checked in the strings). We **make `dlopen("libOpenSLES.so")` fail**, so OpenAL selects the **AudioTrack backend**. That backend uses plain JNI, not BridgeKit, and needs about 7 members of a fake `android.media.AudioTrack`:
  - `getMinBufferSize`, `<init>`, `play`, `stop`, `release`, `write([BII)` and `flush`.
  - `write` pushes PCM into an SDL audio queue: 44.1 kHz, s16, stereo.

  This takes about 1 week. A real OpenSL ES shim (about 25 vtable functions) is the fallback only if latency is bad.
- **Music.** There are 9 `.mp4` (AAC) files played through `com.apportable.media.MediaPlayer`. We decode them with **symphonia** (pure Rust; `isomp4` + `aac` features; MPL-2.0, which is GPL-3 compatible) and mix them into the same SDL stream. The `MediaPlayer` callbacks (`onPrepared`, `onCompletion` and so on) are posted to MainThread.
- The 152 WAV sound effects go through libAudioFile and OpenAL in guest code, so we do nothing for them.

### 1.7 Input mapping (window to UIKit touches)

| Host input | Guest event |
|---|---|
| Left button down/drag/up | `Window.nativeTouchesBegin/Move/End`, pointer id 0, coordinates scaled from window pixels to the content size |
| Right drag | 2-finger pan (two synthetic touches moving together) |
| Mouse wheel / trackpad pinch / Ctrl+drag | 2-finger pinch zoom: synthetic touches mirrored around the cursor, with the step size tunable |
| Long press (hold LMB 500 ms) | Native to the game; no mapping needed |
| Keyboard while a TextField is focused | `SDL_StartTextInput`, then `TextField.shouldChangeCharactersInRange`, `textDidChange`, then `shouldReturn` on Enter |
| Esc | Android Back (`Window.nativeKeyDown/Up` with `KEYCODE_BACK`) |
| Window focus and minimise | `nativeOnWindowFocusChanged`, `Lifecycle.applicationWillResignActive/DidEnterBackground` and the reverse |
| Resize / fullscreen (F11) | `nativeOnSurfaceChanged`, `nativeRootGeometryChanged` |

Content scale: the default makes the window behave like a 1024x768-point iPad-class surface. This is configurable because the UI is built for phone and tablet sizes.

### 1.8 File storage
- Data root:
  - Linux: `$XDG_DATA_HOME/blockheads-pc` (default `~/.local/share/blockheads-pc`)
  - Windows: `%APPDATA%\blockheads-pc`
  - macOS: `~/Library/Application Support/blockheads-pc`

  The path comes from the `directories` crate.
- Layout:
  - `home/` is the guest `HOME` and files dir, where saves and worlds land.
  - `cache/` and `tmp/` hold scratch data.
  - `prefs.json` backs SharedPreferences.
  - `config.toml` holds keybinds, scale and the APK path.
  - `logs/` holds the guest log and the JNI trace.
- The APK is **never extracted**. Libraries are read from zip entries into the guest window, and libv's zzip reads `assets/` straight from the APK. Only the APK's path and SHA-256 are stored.
- Later (M9): import saves from an Android device via `adb backup`/`run-as` export, as user-initiated tooling.

## 2. Reuse

| Asset | Reuse |
|---|---|
| `blockheads-patcher/packages/core/src/zip` (TS, 388 LOC with the ELF helper) | Not linkable from Rust. We reuse its **test fixtures and edge cases** (data descriptors, stored versus deflated entries, `zipalign`). The Rust side uses the `zip` crate (MIT), or a port of `reader.ts` of about 150 LOC if we want zero dependencies. |
| `blockheads-patcher` **native-stub patch spec** (`makeNativeStubPatch`, which checks a library SHA-256 and the original words, then writes `bx lr`) | **Directly reusable as a concept and as data.** We port the spec to Rust as an *in-memory* patch applied after mapping. Patches stay per-version and hash-gated. `fix-iap-store-font-crash` applies as is, since the same native code runs. `fix-welcome-popup-deadlock` is a manifest patch and does not apply on PC (section 1.4). |
| `blockheads-server` (clean-room 1.7.1 server, Rust) | It is the M7 multiplayer target. Its README already states that Android 1.7.x clients connect after the patcher's popup fix, which lowers C's protocol-mismatch risk. |
| `blockheads-server/crates/bh_testclient` (`trace.rs`, `decode.rs`, harness) | It decodes and diffs ENet/protocol traffic from the PC client against an Android-client trace, which is the M7 exit check. |
| REA/Frida tooling with adb (development only) | A **reference trace** from a real Android device: the JNI `FindClass/GetMethodID/Call*` sequence and GL call stream during boot and menu. This is a dev oracle and does not ship. It replaces the qemu-user oracle from C, which would need a full glibc-to-bionic shim of its own. |
| Our Rust conventions (GPL-3.0-or-later, bun, semantic-release, 100% coverage CI) | Applied as is (section 6). |

## 3. Milestones

Effort is in **dev-weeks of focused work**. "Demo" is what we show; "Exit" is the check that must pass to move on. The first gates are deliberately early and cheap.

| # | Name | Demo | Exit criteria | Effort |
|---|---|---|---|---|
| **M0** | JIT spike and census | `bhpc-run fixtures/hello.so` prints `hello from ARM` through a `puts` thunk. The fixture also exercises ARM/Thumb interworking, a NEON loop, VFP double maths with softfp, and a C++ throw/catch that unwinds through `__gnu_Unwind_Find_exidx`. | (1) dynarmic fork builds from `cargo build` on Linux. (2) A **symbol-bounded** ARM/Thumb/VFP/NEON opcode census of all 25 libs (mapping-symbol-free disassembly starting at known function starts) is diffed against the dynarmic decoder: **0 unsupported opcodes, or each one has a fix plan under 1 week in total.** (3) The thunk ABI measures under 200 ns per call. | 1.5 |
| **G0** | Gate | | Kill or switch to the interpreter if NEON or Thumb gaps exceed 1-2 weeks of work. | |
| **M1** | Load, relocate, run constructors | `bhpc --apk <path> --init-only` maps all 25 libs from the APK in DT_NEEDED order, applies about 150k relocations, runs every `.init_array` (159 in libApplication), and prints a thunk-call histogram. A host-driven smoke test then calls `objc_getClass("UIApplication")`, `CFStringCreateWithCString` and `NSLog(@"…")` in the guest. | All constructors return. The ObjC classes are registered. `NSLog` output reaches the host log. Unimplemented imports hit during init: **0**. | 3 |
| **G1** | Gate (**main go/no-go**) | | If M1 slips more than 2 weeks past plan because of the ObjC runtime, EHABI or jemalloc, re-estimate everything at ×2 or stop. | |
| **M2** | bionic core and threads | Host tests run guest libdispatch (`dispatch_async` to a global queue, `dispatch_after`, groups and semaphores). A guest `CFRunLoop` runs 2 s with a timer and a socket source. A guest `NSFileManager` lists `assets/` through libv zzip from the APK. | The libdispatch test is stable under ×100 repetition (no deadlock or race; TSAN on the host side). The struct-layout tests pass. The printf family matches bionic output on 2,000 generated format cases (an NDK fixture runs the same cases in the guest and compares). | 3.5 |
| **M3** | Fake JVM and `UIApplicationMain` | The host-driven boot sequence (section 1.4) reaches the game's `applicationDidFinishLaunching` (logged), and the unified run loop idles. There is no GL yet: the surface callbacks are deferred. | The run loop has been stable for 60 s. Every `FindClass`/`GetMethodID` in the boot trace is classified as *implemented* or *intentionally stubbed*. No abort-slot JNI functions are hit. The trace is diffed against the Android device reference trace. | 3.5 |
| **G3** | Gate | | If the BridgeKit/DexMaker proxy path cannot be made to work, evaluate replacing libBridgeKit with a host ObjC-class library (B §7.1) before continuing. That would add about 4 weeks. | |
| **M4** | First frame (Linux) | An SDL window shows the **title/menu screen**, rendered by the game's own shaders on Mesa GLES2. | Pixel-compared to an Android screenshot at the same logical size (SSIM ≥ 0.95, allowing for font hinting). At least 60 fps at 1080p. The GL error log is empty. | 3.5 |
| **M5** | Interactive menu | The mouse navigates the menus, the keyboard enters a world name, and a new world is **created and saved**. Settings persist across a restart. | A world file exists under `home/`. Touch, text and Back flows work. AlertView, Toast and the welcome message display. No unimplemented-member log lines in these flows. | 2.5 |
| **M6** | Single-player gameplay and audio | A 30-minute session: walk, dig, craft, place, day/night, with sound effects and music. The world reloads after quit. | A 30-minute soak with no crash. p99 frame time under 16.7 ms on the dev box (Ryzen 9 5980HS) and under 33 ms on a 4-core 2018 laptop. No audible underruns. Save and load round-trips. | 5 |
| **M7** | Multiplayer | Two PC clients join a local `blockheads-server` world, chat, and see each other build. One Android client joins the same server. | The `bh_testclient` trace diff shows the same handshake and packet stream shape as the Android client. A 15-minute session has no desync or disconnect. HTTP calls (if any are used for server lists or news) are stubbed or working. | 1.5 |
| **M8** | Windows and macOS | The same M6 demo on Windows 11 (ANGLE/D3D11) and macOS 14+ on Intel and Apple Silicon (ANGLE/Metal, dynarmic arm64, `MAP_JIT`). | The M6 exit criteria pass on all three OSes. CI builds signed artifacts for each. On Windows, the virtual-fd epoll and socket paths pass the M2 test suite. | 4 |
| **M9** | Versions and polish | 1.7.5 and 1.7.6 APKs boot, with patches keyed by library hash. Keybind UI, fullscreen, launcher (APK picker, data dir), Android save import. | All three APK versions pass M5/M6 smoke tests. First `v0.1.0-dev.N` prerelease. | 3 |
| | **Total** | | | **31 (26-42)** |

Calendar: about 8 months at roughly 1 FTE, or 12-15 months part time. Visible progress (the M4 window) arrives at about week 15. The money is spent mostly *after* G1, and G1 comes at about week 4.5.

## 4. Top risks and mitigations

| # | Risk | Likelihood / impact | Mitigation |
|---|---|---|---|
| R1 | The **BridgeKit reflection bridge** (runtime signatures, DexMaker proxies, `JavaThrowableLandingPad`) is harder to fake than counted. B's numbers carry ±40% error bars. | High / High | Never-fail `GetMethodID` plus a trace-driven work queue. Use a device reference trace to know the target set early. Fallback: replace libBridgeKit with a host ObjC wrapper library (G3). |
| R2 | **Looper and CFRunLoop unification** has subtle semantics, leading to hangs, starvation or ordering bugs. | Medium / High | Copy the AOSP `MessageQueue` semantics (timed queue, sync barriers, idle handlers). Use the Android trace for ordering. Watchdog logging when MainThread blocks for more than 1 s. |
| R3 | **dynarmic** has NEON or Thumb-2 gaps, or the fork goes stale (upstream was taken down per C; not re-verified here). | Medium / High | The M0 census happens before any other work. Vendor and pin a fork and keep our patches small. The interpreter fallback is the kill-or-continue decision at G0. |
| R4 | **Bionic ABI mismatches** (FILE, pthread words, `stat`/`dirent`/`addrinfo` layouts, `va_list`) cause silent memory corruption. | High / Medium | Layout unit tests per struct. NDK-built differential fixtures that run the same C code as guest and compare. Guard pages around the guest arena. A debug mode that logs every thunk with its arguments. |
| R5 | **Threading and memory model**: x86 TSO versus ARM weak ordering hides or creates races; lazy mutex tables race. | Medium / Medium | x86 TSO is *stronger* than ARM, so guest races rarely appear on x86. On Apple Silicon, rely on dynarmic's barrier emission. Lazy mutex creation uses one global `DashMap` keyed by address. |
| R6 | **GL**: ES1 entry points are actually used on the ES2 path; driver differences; the shared-context threading rules on macOS. | Medium / Medium | Use ANGLE everywhere except Mesa Linux, which gives one behaviour. ANGLE's GLES1 front end covers ES1. Keep all GL work on the GLThread unless the trace proves otherwise. |
| R7 | **Debuggability**: the libs are stripped and the custom ObjC runtime makes crashes opaque. | High / Medium | A guest backtrace through the `.ARM.exidx` unwinder, with symbolisation from dynsym and ObjC method lists (B already has a metadata parser). A gdb-remote stub on the Jit, about 1 week, built when first needed. A thunk trace ring buffer is dumped on crash. |
| R8 | **Performance**: JIT warm-up for a 17 MB lib, CoreGraphics and Pango software paths, thunk-heavy GL. | Low / Medium | Hot libc functions (`memcpy`, `memset`, `strlen`, `memcmp`) run as host thunks. The block cache persists across runs (later). Profile with `perf` plus JIT perf-map output. |
| R9 | **Version drift** across 1.7.4, 1.7.5 and 1.7.6 (library hashes differ: libApplication is 17,176,184, 17,175,072 and 17,174,960 B). | Low / Low | Use no hard-coded addresses except hash-gated patches. Everything else binds by symbol name or JNI name. |
| R10 | **macOS and Windows platform friction**: JIT entitlements, notarisation, ANGLE builds, Winsock. | Medium / Low | Linux first. The virtual fd table removes the epoll and pipe dependence. ANGLE is built once in CI and cached. macOS uses the `com.apple.security.cs.allow-jit` entitlement. |
| R11 | **Legal or perception risk** from users sharing "ready to run" bundles. | Low / High | The tool refuses to start without an APK whose lib hashes match a known release. It has no download feature and no extracted output. Docs and the README carry the non-affiliation and no-redistribution notices. |

## 5. Legal notes
- **User-supplied input only.** The user points the app at their own APK, obtained from a store they bought it from or from their own device. The app checks known library SHA-256s and refuses unknown builds instead of guessing.
- **No redistribution, ever.**
  - No game code, assets, extracted libraries, decompiled code or Apportable runtime in the repo, CI caches or release artifacts.
  - CI never downloads an APK.
  - Integration tests that need the APK run only locally, behind `BHPC_APK=/path` and `#[ignore]`.
- **Patches are data about the user's file**: library hash, virtual address and expected original word. This is the same model `blockheads-patcher` already uses. No game bytes are embedded.
- **Clean-room host code.** Host implementations follow public API docs: bionic headers in the NDK, Android SDK docs, the JNI spec, Khronos GLES and OpenSL specs. The private RE notes in `~/Projects/reverse-engineering` stay out of the repo, per the org rules in `AGENTS.md`.
- **Licences.**

  | Component | Licence | Status |
  |---|---|---|
  | blockheads-pc | GPL-3.0-or-later | Project licence |
  | dynarmic fork | 0BSD | Compatible |
  | xbyak, oaknut, fmt, mcl | BSD/MIT | Compatible |
  | SDL2 | zlib | Compatible |
  | ANGLE | BSD-3 | Compatible |
  | symphonia | MPL-2.0 | Compatible with GPL-3 |
  | mio, socket2, ureq, rustls, parking_lot, zip, directories | MIT/Apache-2.0 | Compatible |
  | unicorn, QEMU | GPL-2.0-only | **Excluded** from linking |

- **Trademarks.** "The Blockheads" belongs to its owners. Use a neutral product name such as "Blockheads PC Loader" in descriptive use only, no game logo or art, and include the standard "not affiliated with Majic Jungle / Noodlecake" notice. Interoperability with a lawfully owned copy is the stated purpose. This is not legal advice; get a review before the first public release.

## 6. Engineering rules for the repo
- Org rules apply: GPL-3.0-or-later, Conventional Commits, `dev` branch prereleases, and **100% coverage enforced in CI**.
- How the coverage rule stays satisfiable without the APK:
  - The pure-Rust crates (`bhpc-elf`, `bhpc-mem`, `bhpc-bionic`, `bhpc-jni`, the generated GL marshalling) are unit-tested against **NDK-built fixture `.so` files compiled from our own C** in CI (NDK r27 clang `armv7a-linux-androideabi21`).
  - Those fixtures exercise relocations, Thumb interworking, softfp, varargs, pthreads, stdio macros, setjmp, C++ EH and JNI round trips.
  - The dynarmic C++ shim and the SDL window shell are excluded from coverage by a documented `cfg(coverage)` boundary, the same pattern as platform shells in the other repos. The exclusion list is kept short and reviewed.
- Logging uses `tracing` with per-subsystem targets: `guest`, `thunk`, `jni`, `gl`, `vfs`.
- `--trace-jni` and `--trace-thunks` are permanent features, not temporary hacks.

## 7. Decision requested
1. Approve **M0 and M1 (about 4.5 dev-weeks)** with the G0 and G1 gates as written.
2. Approve the dependency choices: dynarmic fork (0BSD), SDL2, ANGLE, symphonia.
3. Approve capturing a **reference JNI and GL trace on a real Android device** (dev-only, adb plus Frida), to drive M3 to M5.

## 8. Not in scope (now)
- No Android emulator, no ART or dex execution, and no running of the Java code. All Java is replaced.
- No iOS build and no rewriting of game logic. The guest code runs unchanged except for hash-gated stubs.
- No ads, IAP or Play Games functionality.
- No net-new features beyond input mapping and quality-of-life settings.

## 9. Verification of sub-report claims (spot checks on 1.7.4)

| Claim | Check | Result |
|---|---|---|
| 25 native libs (C says 26, B says 27) | `unzip -l lib/*` | **25 `.so`, 48,544,088 B.** C and B miscounted; the extra file in the scratch dir is `ext_imports.txt`. |
| softfp, VFPv3 + NEONv1 in libApplication | `readelf -h/-A` | Confirmed: `Flags 0x5000000` (no hard-float), `Tag_FP_arch: VFPv3`, `Tag_Advanced_SIMD_arch: NEONv1`. |
| `Java_*` exports: 167 = 90 + 77 (C says 169 = 91 + 78) | `readelf --dyn-syms` | **90 libApplication + 77 libBridgeKit = 167.** A is right. |
| Only EGL import is `eglGetCurrentContext` | dynsym of libgles_apportable | Confirmed. |
| libApplication does no direct JNI lookups | strings | Confirmed: only `AndroidFindClass` (a BridgeKit wrapper name). BridgeKit holds 7 `dexmaker` strings. |
| OpenAL backends opensles, then audiotrack, then null; `dlopen /system/lib/libOpenSLES.so` | strings of libOpenAL | Confirmed, and it underpins the section 1.6 choice. |
| ObjC runtime and libdispatch are guest code in libSystem | dynsym | Confirmed: `objc_msgSend`, `objc_exception_throw`, `dispatch_async`, `_dispatch_main_q` are defined in libSystem.so. |
| `DT_FLAGS=SYMBOLIC`, `NEEDED libstdc++.so` in libApplication | `readelf -d` | Confirmed. Treat `libstdc++.so` as an empty module. |
| ES2 shaders in assets | assets `*.fsh` | Confirmed: GLSL ES with `highp` and `lowp` precision (2011 iOS-era shaders). |
| ENet statically linked | strings | Confirmed: 66 `enet_*` strings in libApplication. B's count of 159 includes other matches. |
| Later versions keep the same ABI layout | `unzip -l` of 1.7.5 and 1.7.6 | Both are armeabi-v7a only, 25 libs. The libApplication size differs by about 1 KB per version. |
| Upstream dynarmic removed and forks alive | not re-verified offline | **Unverified.** Re-check in M0 before vendoring. |
