# A. Native dependency surface of The Blockheads 1.7.4 (armeabi-v7a)

Source: `The Blockheads_1.7.4_APKPure.apk`, `lib/armeabi-v7a/` only (the APK ships a single ABI: ARM32 EABI5, hard-float not used, soft/softfp). Extracted with unzip into a scratch dir; analysed with pyelftools/readelf (static only, nothing executed). No game code or assets are redistributed by this document: only symbol names and counts.

## 1. Headline numbers

* **25** shared objects, 48.5 MB total, all `EM_ARM`, all `ET_DYN` with `PT_INTERP=/system/bin/linker`, ARM_EXIDX unwind tables (C++ EH is table based, so a host loader must register `.ARM.exidx` for `__gnu_Unwind_Find_exidx`).
* **5580** undefined-symbol entries across all libs (counted per lib): 3656 resolve to a symbol defined by a different bundled lib, the other 1924 are self-defined or not defined anywhere in the bundle. Distinct symbols defined nowhere in the bundle (must come from Android/bionic or our shim): **764**.
* Distinct system imports by group: libjnigraphics: 3, bionic-specific ABI / ARM runtime helpers: 31, liblog: 3, libm: 68, libc: stdlib / env / heap: 12, libc: socket / net / poll / epoll / inotify: 37, libc: file / fd / dir / mmap: 46, libc: string / memory / wchar / sort: 47, libc: stdio (FILE*): 30, libc: time / timezone: 16, libc: process / signal / identity / syslog / sys: 28, libdl: 5, EGL (libEGL): 1, GLES (libGLESv1_CM / libGLESv2): 380, setjmp family: 4, libc: pthread / sched / sleep: 53. Sum = 764.
* **Bundled** (we do NOT need to emulate): jemalloc + zlib + Itanium unwinder + BSD stdio helpers (`libv.so`, 227 JUMP_SLOT/1019 exports), libc++ (`libcxx.so`), libdispatch/libobjc-like runtime (`libSystem.so`: 95 `objc_*`, 95 `dispatch_*`, 36 `class_*`), ICU 50, libxml2, OpenSSL (in libSecurity/libApplication), pango/glib, Skia-like CoreGraphics, OpenAL-soft, enet, etc.
* **System libraries actually required**: `libc` (bionic), `libm`, `libdl`, `liblog`, `libGLESv1_CM`, `libGLESv2`, `libEGL` (exactly **1** symbol: `eglGetCurrentContext`), `libjnigraphics` (3 symbols, `AndroidBitmap_*`), and `libOpenSLES` (not in DT_NEEDED; `dlopen`ed by OpenAL-soft and AudioUnit at runtime via `/system/lib/libOpenSLES.so`, `slCreateEngine`, `SL_IID_{ENGINE,PLAY,BUFFERQUEUE,ANDROIDSIMPLEBUFFERQUEUE,RECORD}`).
* **Not used at all**: `libandroid.so` (no `AAssetManager`/`ANativeWindow`/`ALooper` etc.; assets, window and input go through Java -> JNI), `libz.so` (zlib is statically bundled in libv/libApplication), `libstdc++.so` (listed as NEEDED by libApplication but contributes zero unresolved C++ symbols; only ARM EHABI helpers `__cxa_begin_cleanup`/`__cxa_type_match` are unresolved).
* JNI: **167** `Java_*` exports (libApplication 90, libBridgeKit 77) + `JNI_OnLoad` in libApplication, libBridgeKit, libOpenAL, libv (+`JNI_OnUnload` in libOpenAL). See section 6.

## 2. Per-library table

Sorted by size. "Undef" = all undefined dynsym entries; "ext" = distinct ones not defined by any bundled lib (i.e. must come from the system / our shim); "Exports" = defined dynsym entries.

| Library | Size (B) | DT_NEEDED | Undef | ext (system) | Exports |
|---|---:|---:|---:|---:|---:|
| libApplication.so | 17176184 | 27 | 828 | 145 | 11271 |
| libicu.so | 4351952 | 5 | 111 | 65 | 10655 |
| libpango.so | 3942256 | 7 | 258 | 126 | 4284 |
| libCoreGraphics.so | 3399676 | 9 | 351 | 81 | 9554 |
| libCoreFoundation.so | 3299448 | 7 | 447 | 97 | 2079 |
| libSystem.so | 2712656 | 6 | 259 | 159 | 2153 |
| libGraphicsServices.so | 2254056 | 5 | 120 | 68 | 356 |
| libFoundation.so | 2180364 | 10 | 920 | 98 | 672 |
| libxml2.so | 1642232 | 5 | 65 | 46 | 1484 |
| libSecurity.so | 1628112 | 6 | 160 | 58 | 4065 |
| libcxx.so | 1576732 | 2 | 153 | 60 | 2914 |
| libBridgeKit.so | 1404948 | 9 | 127 | 33 | 964 |
| libv.so | 839996 | 4 | 233 | 231 | 1019 |
| libCommonCrypto.so | 534164 | 3 | 40 | 21 | 406 |
| libCFNetwork.so | 499984 | 9 | 452 | 48 | 578 |
| libOpenAL.so | 342348 | 5 | 91 | 65 | 273 |
| libgles_apportable.so | 279620 | 8 | 428 | 394 | 546 |
| libAudioFile.so | 172004 | 6 | 63 | 36 | 45 |
| libCoreText.so | 137896 | 10 | 250 | 12 | 327 |
| libAudioUnit.so | 88280 | 8 | 52 | 28 | 84 |
| libAudioToolbox.so | 29220 | 8 | 58 | 20 | 97 |
| libSystemConfiguration.so | 20440 | 4 | 59 | 16 | 62 |
| libNoodleFoundation.so | 14252 | 5 | 26 | 6 | 29 |
| libNoodleCompatibility.so | 10400 | 6 | 25 | 8 | 31 |
| libCoreAudio.so | 6868 | 1 | 4 | 3 | 30 |

## 3. DT_NEEDED graph

### 3.1 Bundled-to-bundled edges (in-APK) and system edges

| Library | Needs (bundled) | Needs (system) |
|---|---|---|
| libApplication.so | libAudioFile.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCFNetwork.so, libCoreAudio.so, libCoreFoundation.so, libCoreGraphics.so, libCoreText.so, libFoundation.so, libNoodleCompatibility.so, libNoodleFoundation.so, libOpenAL.so, libSecurity.so, libSystemConfiguration.so, libv.so, libcxx.so, libSystem.so, libCommonCrypto.so, libgles_apportable.so | libGLESv1_CM.so, libGLESv2.so, liblog.so, libstdc++.so, libm.so, libdl.so, libc.so |
| libAudioFile.so | libv.so, libSystem.so, libCoreFoundation.so, libBridgeKit.so | liblog.so, libc.so |
| libAudioToolbox.so | libv.so, libSystem.so, libBridgeKit.so, libAudioUnit.so, libOpenAL.so | liblog.so, libdl.so, libc.so |
| libAudioUnit.so | libv.so, libSystem.so, libCoreFoundation.so, libBridgeKit.so, libAudioFile.so | libm.so, libdl.so, libc.so |
| libBridgeKit.so | libv.so, libSystem.so, libCoreFoundation.so, libCFNetwork.so, libFoundation.so | liblog.so, libjnigraphics.so, libdl.so, libc.so |
| libCFNetwork.so | libv.so, libSystem.so, libicu.so, libCoreFoundation.so, libCommonCrypto.so, libSecurity.so, libSystemConfiguration.so | liblog.so, libc.so |
| libCommonCrypto.so | libv.so, libSystem.so | libc.so |
| libCoreAudio.so |  | libc.so |
| libCoreFoundation.so | libv.so, libSystem.so, libicu.so | liblog.so, libm.so, libdl.so, libc.so |
| libCoreGraphics.so | libv.so, libcxx.so, libSystem.so, libGraphicsServices.so, libCoreFoundation.so, libxml2.so | liblog.so, libm.so, libc.so |
| libCoreText.so | libv.so, libSystem.so, libCoreFoundation.so, libFoundation.so, libGraphicsServices.so, libCoreGraphics.so, libpango.so | liblog.so, libm.so, libc.so |
| libFoundation.so | libcxx.so, libv.so, libSystem.so, libicu.so, libCoreFoundation.so, libCFNetwork.so, libxml2.so | liblog.so, libm.so, libc.so |
| libGraphicsServices.so | libv.so, libSystem.so, libxml2.so | libm.so, libc.so |
| libNoodleCompatibility.so | libv.so, libSystem.so, libCommonCrypto.so, libCoreFoundation.so, libFoundation.so | libc.so |
| libNoodleFoundation.so | libv.so, libSystem.so, libCoreFoundation.so, libFoundation.so | libc.so |
| libOpenAL.so | libv.so | liblog.so, libm.so, libdl.so, libc.so |
| libSecurity.so | libv.so, libSystem.so, libCommonCrypto.so, libCoreFoundation.so | libdl.so, libc.so |
| libSystem.so | libv.so, libcxx.so | liblog.so, libm.so, libdl.so, libc.so |
| libSystemConfiguration.so | libv.so, libSystem.so, libCoreFoundation.so | libc.so |
| libcxx.so | libv.so | libc.so |
| libgles_apportable.so | libv.so, libcxx.so, libSystem.so | libEGL.so, libGLESv1_CM.so, libGLESv2.so, liblog.so, libc.so |
| libicu.so | libv.so, libcxx.so | libm.so, libdl.so, libc.so |
| libpango.so | libv.so, libcxx.so, libSystem.so, libGraphicsServices.so | libm.so, libdl.so, libc.so |
| libv.so |  | liblog.so, libm.so, libdl.so, libc.so |
| libxml2.so | libv.so, libSystem.so | libm.so, libdl.so, libc.so |

### 3.2 Who needs which system lib

| System lib | # bundled libs listing it | Listed by |
|---|---:|---|
| libEGL.so | 1 | libgles_apportable.so |
| libGLESv1_CM.so | 2 | libApplication.so, libgles_apportable.so |
| libGLESv2.so | 2 | libApplication.so, libgles_apportable.so |
| libc.so | 25 | libApplication.so, libAudioFile.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, ... (25) |
| libdl.so | 12 | libApplication.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCoreFoundation.so, ... (12) |
| libjnigraphics.so | 1 | libBridgeKit.so |
| liblog.so | 13 | libApplication.so, libAudioFile.so, libAudioToolbox.so, libBridgeKit.so, libCFNetwork.so, ... (13) |
| libm.so | 13 | libApplication.so, libAudioUnit.so, libCoreFoundation.so, libCoreGraphics.so, libCoreText.so, ... (13) |
| libstdc++.so | 1 | libApplication.so |

Layering (leaf to root): `libc/libm/libdl/liblog` -> `libv` (jemalloc, zlib, unwind, wrappers) -> `libcxx` -> `libSystem` (objc runtime, libdispatch, BSD libc extras, malloc zones) -> `libicu`, `libxml2`, `libCoreFoundation` -> `libFoundation`, `libCFNetwork`, `libCommonCrypto`, `libSecurity` -> `libGraphicsServices`, `libCoreGraphics`, `libpango`, `libCoreText` -> `libBridgeKit` (Java UIKit bridge) -> `libAudio*`/`libOpenAL` -> `libApplication` (game + UIKit/AppKit reimplementation). `libgles_apportable` sits beside the graphics stack and is the sole consumer of libEGL/GLES. Every bundled lib has **weak undef `__load_bridge`** (Apportable hook, resolved by the Java-side loader; we must provide or leave NULL).

## 4. System imports by group (counts and consumers)

| Group | Distinct symbols | Used by (bundled libs) |
|---|---:|---|
| libjnigraphics | 3 | libBridgeKit.so |
| bionic-specific ABI / ARM runtime helpers | 31 | 25 libs, incl. libApplication.so, libAudioFile.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCFNetwork.so |
| liblog | 3 | 13 libs, incl. libApplication.so, libAudioFile.so, libAudioToolbox.so, libBridgeKit.so, libCFNetwork.so, libCoreFoundation.so |
| libm | 68 | 13 libs, incl. libApplication.so, libAudioUnit.so, libCoreFoundation.so, libCoreGraphics.so, libCoreText.so, libFoundation.so |
| libc: stdlib / env / heap | 12 | 24 libs, incl. libApplication.so, libAudioFile.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCFNetwork.so |
| libc: socket / net / poll / epoll / inotify | 37 | 10 libs, incl. libApplication.so, libBridgeKit.so, libCFNetwork.so, libCoreFoundation.so, libFoundation.so, libSecurity.so |
| libc: file / fd / dir / mmap | 46 | 14 libs, incl. libApplication.so, libAudioFile.so, libCFNetwork.so, libCommonCrypto.so, libCoreFoundation.so, libCoreGraphics.so |
| libc: string / memory / wchar / sort | 47 | 24 libs, incl. libApplication.so, libAudioFile.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCFNetwork.so |
| libc: stdio (FILE*) | 30 | 19 libs, incl. libApplication.so, libAudioFile.so, libAudioUnit.so, libBridgeKit.so, libCFNetwork.so, libCommonCrypto.so |
| libc: time / timezone | 16 | 15 libs, incl. libApplication.so, libAudioToolbox.so, libBridgeKit.so, libCFNetwork.so, libCoreFoundation.so, libCoreGraphics.so |
| libc: process / signal / identity / syslog / sys | 28 | 11 libs, incl. libApplication.so, libCFNetwork.so, libCommonCrypto.so, libCoreFoundation.so, libFoundation.so, libGraphicsServices.so |
| libdl | 5 | 12 libs, incl. libApplication.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCoreFoundation.so, libOpenAL.so |
| EGL (libEGL) | 1 | libgles_apportable.so |
| GLES (libGLESv1_CM / libGLESv2) | 380 | libApplication.so, libgles_apportable.so |
| setjmp family | 4 | libCoreGraphics.so, libGraphicsServices.so, libv.so |
| libc: pthread / sched / sleep | 53 | 18 libs, incl. libApplication.so, libAudioFile.so, libAudioToolbox.so, libAudioUnit.so, libBridgeKit.so, libCFNetwork.so |

Full symbol lists (with per-symbol importing libs) are in the companion file `A-native-imports-symbols.txt`. Condensed lists follow.

### libjnigraphics (3)

`AndroidBitmap_getInfo AndroidBitmap_lockPixels AndroidBitmap_unlockPixels`

### bionic-specific ABI / ARM runtime helpers (31)

`__aeabi_memcpy __aeabi_memmove __aeabi_memset __assert2 __cxa_atexit __cxa_begin_cleanup __cxa_finalize __cxa_type_match __errno __futex_wait __futex_wake __get_h_errno __gnu_Unwind_Find_exidx __load_bridge __page_size __sF __srefill __srget __stack_chk_fail __stack_chk_guard __swbuf __system_property_get _ctype_ _tolower_tab_ bsd_signal cacheflush dirname_r fgetln funopen pthread_cond_timedwait_monotonic pthread_cond_timedwait_relative_np`

### liblog (3)

`__android_log_print __android_log_vprint __android_log_write`

### libm (68)

`__fpclassifyd __isfinitef __isinf __isinff __isinfl __isnanl __signbit acos acosf acosh asin asinh atan atan2 atan2f atanf atanh cbrt ceil ceilf cos cosf cosh erf erfc exp exp2 expf expm1 floor floorf fmax fmaxf fmin fminf fmod frexp hypot isnan isnanf ldexp lgamma log log10 log1p logb logf lround lroundf modf nearbyint pow remainder remquo rint round roundf scalbn sin sinf sinh sqrt sqrtf tan tanf tanh tgamma trunc`

### libc: stdlib / env / heap (12)

`_exit abort calloc environ exit free getenv lrand48 putenv setenv srand48 unsetenv`

### libc: socket / net / poll / epoll / inotify (37)

`accept bind connect epoll_create epoll_ctl epoll_wait getaddrinfo gethostbyaddr gethostbyname2 gethostname getnameinfo getpeername getsockname getsockopt if_indextoname if_nametoindex inet_nsap_addr inet_nsap_ntoa inet_ntoa inet_ntop inet_pton inotify_add_watch inotify_init listen poll pselect recv recvfrom recvmsg select send sendmsg sendto setsockopt shutdown socket socketpair`

### libc: file / fd / dir / mmap (46)

`access chmod close closedir creat dirfd dup dup2 fchdir fchmod fchown fcntl flock fnmatch fstat fstatfs fsync ftruncate ioctl isatty lseek lstat madvise mkdir mmap msync munmap open opendir pipe pwrite read readdir readdir_r readlink remove rename rmdir statfs symlink umask unlink utime utimes write writev`

### libc: string / memory / wchar / sort (47)

`atoi atol bcopy bsearch bzero iswalnum iswdigit iswspace iswxdigit memchr memcmp memcpy memmem memmove memset qsort strcasecmp strcasestr strcat strchr strcmp strcoll strcpy strcspn strerror strerror_r strlcat strlcpy strlen strncasecmp strncat strncmp strncpy strnlen strpbrk strrchr strsignal strspn strstr strtok_r strtol strtoll strtoul strtoull strxfrm towlower towupper`

### libc: stdio (FILE*) (30)

`clearerr fclose fflush fgetc fgets fopen fprintf fputc fputs fread freopen fscanf fseek ftell fwrite getc perror putc rewind setbuf setvbuf snprintf sprintf sscanf ungetc vfprintf vprintf vsnprintf vsprintf vsscanf`

### libc: time / timezone (16)

`clock clock_gettime getitimer gettimeofday gmtime gmtime_r localtime localtime_r mktime setitimer strftime strptime time timezone tzname tzset`

### libc: process / signal / identity / syslog / sys (28)

`closelog execv execve fork getegid geteuid getgid getpid getppid getrlimit getrusage getuid initgroups kill openlog prctl raise setpriority setrlimit setuid sigaction sigprocmask sigsuspend syscall sysconf uname vsyslog waitpid`

### libdl (5)

`dladdr dlclose dlerror dlopen dlsym`

### EGL (libEGL) (1)

`eglGetCurrentContext`

### setjmp family (4)

`longjmp setjmp siglongjmp sigsetjmp`

### libc: pthread / sched / sleep (53)

`nanosleep pthread_attr_destroy pthread_attr_getdetachstate pthread_attr_getguardsize pthread_attr_getschedparam pthread_attr_getschedpolicy pthread_attr_getstack pthread_attr_getstackaddr pthread_attr_getstacksize pthread_attr_init pthread_attr_setdetachstate pthread_attr_setguardsize pthread_attr_setschedparam pthread_attr_setschedpolicy pthread_attr_setscope pthread_attr_setstack pthread_attr_setstackaddr pthread_attr_setstacksize pthread_cond_broadcast pthread_cond_destroy pthread_cond_init pthread_cond_signal pthread_cond_timedwait pthread_cond_wait pthread_condattr_destroy pthread_condattr_init pthread_condattr_setpshared pthread_create pthread_detach pthread_equal pthread_exit pthread_getspecific pthread_join pthread_key_create pthread_kill pthread_mutex_destroy pthread_mutex_init pthread_mutex_lock pthread_mutex_trylock pthread_mutex_unlock pthread_mutexattr_destroy pthread_mutexattr_init pthread_mutexattr_setpshared pthread_mutexattr_settype pthread_once pthread_self pthread_setspecific pthread_sigmask sched_get_priority_max sched_get_priority_min sched_yield sleep usleep`

### GLES breakdown

* **380** distinct GL entry points imported, imported mostly by `libgles_apportable.so` (379, incl. the single EGL symbol) plus 5 direct calls from libApplication (`glFrustumf glOrthof glGetError glGetRenderbufferParameteriv(OES)`) -> fixed-function ES1 maths helper plus error checks. libgles_apportable also exports **378** `__wrap_gl*` hooks (debug/state-cache layer; libApplication/others import `__wrap_gl*`).
* Core (non-extension) entry points: **228**; extension-suffixed: **152**. Suffix split: AMD 11, APPL 2, EX 3, EXT 3, IMG 4, NV 9, OES 102, QCOM 18.
* Fixed-function (GLES 1.1) style symbols present (matrix stack, glVertexPointer/ColorPointer/TexCoordPointer, lights, fog, texenv, clip planes, fixed-point `*x`/`*xOES` variants): about **80** core ones; plus fixed-point `x` variants. Programmable (GLES 2.0) symbols (`glCreateShader`, `glUniform*`, `glVertexAttrib*`, `glLinkProgram`, ...): about **61**. Inference (not verified by disassembly): libgles_apportable is a **GLES1+GLES2 wrapper**; the game uses ES2 shaders (strings mention `GL_OES_standard_derivatives` shader warnings) with ES1 fallback. NEEDED: libGLESv1_CM.so and libGLESv2.so (both).
* Extensions referenced as *symbols*: OES framebuffer_object, mapbuffer, vertex_array_object, texture 3D, blend_*_separate, fixed-point; IMG multisampled_render_to_texture + user_clip_plane + PVRTC (`GL_IMG_texture_compression_pvrtc` string); APPLE multisample resolve; QCOM driver-control/tiling/extended-get; AMD perf monitor; NV fence; EXT debug markers/multi_draw. Mapping to a desktop host: all of these are simply *optional* (resolve via `eglGetProcAddress`-style lookup in libgles_apportable; unimplemented entries can be NULL stubs). Real game probably needs well under 120 of them; to be confirmed in analysis B/C.
* Context: `eglGetCurrentContext` is the only EGL symbol; EGL surface/context creation happens on the Java side (GLSurfaceView), which on PC we replace with SDL2/GLFW + ANGLE or a desktop-GL compatibility layer (ES2 on desktop GL 2.1+ is mostly trivial; ES1 fixed-function needs a layer such as gl4es).

## 5. Bionic-specific ABI pitfalls (host loader checklist)

| Topic | Detail |
|---|---|
| errno | `__errno` (function returning `int*`). glibc: `__errno_location`. Map directly; no TLS relocs in the bundle. |
| stdio `FILE` | `__sF` (data symbol, `STT_OBJECT`) imported by **15 libs** (`stdin/stdout/stderr` = `&__sF[0..2]`, sizeof(FILE)=84 on bionic ARM32). Bionic `<stdio.h>` macros `getc/putc/feof/ferror/fileno/__sgetc` inline-access `FILE` fields, and libs call `__srefill`, `__srget`, `__swbuf`, `funopen`, `fgetln` (BSD). Must supply a bionic-layout `FILE` array and the BSD slow-path functions, OR reimplement FILE with identical field layout (_p,_r,_w,_flags,_file,_bf,...). Host `FILE*` pointers cannot be passed straight through when the guest inspects fields. `fopen/fclose/fread/...` are also `__wrap_`ped by libv. `libv` itself implements `__srefill/__swbuf` consumers (it imports them), so the guest bionic-style FILE struct must be the real thing. |
| pthread | `pthread_mutex_t` = 4 bytes (int) on bionic ARM32, `pthread_cond_t` = 4 bytes, `pthread_once_t` = int, `pthread_key_t` = int; glibc sizes are 24/48/4. Guest code statically initialises with `PTHREAD_MUTEX_INITIALIZER`=0 / recursive = `0x4000` tags and embeds mutexes in structs, so the shim must treat the 4-byte guest word as a handle (e.g. side-table keyed by address, lazily created) rather than passing it to host pthreads. Also needed: `pthread_cond_timedwait_monotonic`, `pthread_cond_timedwait_relative_np` (Android-only), `pthread_attr_*stack*/guardsize/sched*`, `pthread_mutexattr_setpshared`, `pthread_condattr_setpshared`, `pthread_sigmask`, `pthread_kill`. Wrapped by libv: `pthread_create/exit/getspecific/setspecific/key_create/key_delete/setname_np/getschedparam/setschedparam` (`__wrap_*`; the real symbols are imported from bionic). `pthread_t` is a pointer-sized opaque; thread stacks must be created by us (guest `pthread_attr_setstack` expected). |
| TLS | No `PT_TLS` segment and **zero** `R_ARM_TLS_*` relocations in all 25 libs: no `__thread`/`thread_local` on the ELF level. TLS is only via `pthread_key_*`. No `__get_tls`/`__aeabi_read_tp` imports, so we do not need to emulate the ARM TLS register (r9/`TPIDRURO`) for these libs. Caveat: inline asm reading CP15 c13 (`mrc p15,0,rX,c13,c0,3`) could exist in libv/jemalloc/libdispatch; needs an instruction scan (not performed here); on x86_64 this requires an ARM interpreter or recompiler anyway. |
| ctype | `_ctype_` (OBJECT; bionic old-style 257-byte table: `_U _L _N _S _P _C _X _B` flag bits, index c+1) used by libSystem, libcxx, libicu; `_tolower_tab_` (CoreGraphics; 257 shorts/bytes table for `tolower` macro). Must be provided as guest-memory tables in bionic layout. `isw*/tow*` are normal functions. |
| stack protector | `__stack_chk_guard` (OBJECT, **20 libs**) and `__stack_chk_fail`. Guard is a global word, not TLS (pre-ICS style); provide a random 32-bit value. |
| setjmp | `setjmp/longjmp/sigsetjmp/siglongjmp`: bionic ARM `jmp_buf` = 64 words (`sigsetjmp` buf additionally stores signal mask); layout is guest-internal only as long as setjmp/longjmp both execute guest side, but if implemented by host functions the buffer size matters (host jmp_buf on x86_64 is 200 B and holds host registers; with an ARM interpreter you must implement these natively on the guest context). libCoreGraphics/libGraphicsServices use setjmp (libpng-style error recovery), libv uses sigsetjmp/siglongjmp. |
| page size / misc data | `__page_size` (OBJECT int) used by libv and CoreGraphics; `environ`, `timezone`, `tzname` data objects; `__get_h_errno`; `__assert2` (bionic assert); `__futex_wait/__futex_wake` (private, libSystem libdispatch semaphores), `__system_property_get` (libSystem), `cacheflush` (BridgeKit, libSystem: JIT/trampolines, ARM syscall 0xf0002; no-op under interpreter), `bsd_signal`, `dirname_r`, `prctl`, `syscall` (libSystem), `epoll_*`/`inotify_*` (libSystem, for CFRunLoop/libdispatch). |
| ARM EHABI | `__gnu_Unwind_Find_exidx` (libv imports it: must scan `PT_ARM_EXIDX` of all loaded guest libs - bionic exports `dl_unwind_find_exidx`), `__cxa_begin_cleanup`, `__cxa_type_match` (personality helpers), `__aeabi_memcpy/memmove/memset` (RTLD must supply ARM RT ABI helpers; bundled libv/libCoreFoundation/pango define others, `__aeabi_*` total 60 in bundle). |
| alloc wrap | 23 of 25 libs import `__wrap_malloc` & friends from libv (link-time `--wrap`). No bundled lib imports plain `malloc` from bionic, and libv embeds jemalloc, so the guest heap does not depend on the host allocator. libv imports `mmap/munmap/madvise/prctl`; the string `/dev/ashmem` appears only in libv (purpose not verified). |
| weak/bridge | `__load_bridge` weak undefined in all 25 libs; leave NULL (relocation `R_ARM_GLOB_DAT` to weak undef = 0) or give an empty stub. |
| DT_FLAGS | libApplication has `DT_FLAGS=SYMBOLIC`; init arrays present (636 B = 159 constructors in libApplication, ObjC `+load` and C++ statics). Lazy PLT (`R_ARM_JUMP_SLOT` 4,6k total) must be supported; `R_ARM_ABS32` and `R_ARM_RELATIVE` dominate (126763 RELATIVE in libApplication). Only 4 reloc types occur: JUMP_SLOT, GLOB_DAT, RELATIVE, ABS32, no TLS, no IRELATIVE, no COPY. |
| filesystem paths | Hard-coded Android paths guest touches: `/proc/cpuinfo`, `/proc/meminfo`, `/proc/%d/{stat,maps,cmdline,fd}`, `/proc/self/{status,cmdline,fd}`, `/sys/class/net/*/address`, `/sys/devices/system/cpu/{possible,present}`, `/system/etc/{hosts,services,protocols,networks}`, `/system/fonts`, `/dev/{random,urandom,ashmem,egd-pool}`, `/etc/localtime`, `/etc/resolv.conf`, `/data` dirs through `ANDROID_DATA_DIRECTORY`/`ANDROID_EXTERNAL_STORAGE` env vars (libv). A path-remapping VFS in the shim covers these; /proc/cpuinfo and /sys cpu files should be synthesised (jemalloc and `android_getCpuCount/Family/Features` in libv read them; ARM feature flags like NEON in cpuinfo must be set consistently with the CPU core used). |

### 5.1 libc symbols wrapped through libv (`--wrap`)

79 non-GL wrappers imported (`__wrap__ZdaPv __wrap__ZdlPv __wrap__Znam __wrap__Znwm __wrap_abort __wrap_accept __wrap_access __wrap_asprintf __wrap_atexit __wrap_calloc __wrap_chdir __wrap_close __wrap_closedir __wrap_exit __wrap_fclose __wrap_fdopen __wrap_fmodf __wrap_fopen __wrap_free __wrap_freeaddrinfo __wrap_fstat __wrap_fsync __wrap_getaddrinfo __wrap_getcwd __wrap_gethostbyaddr __wrap_gethostbyname __wrap_getservbyname __wrap_if_nametoindex __wrap_lseek __wrap_lstat __wrap_malloc __wrap_malloc_usable_size __wrap_mbsrtowcs __wrap_memalign __wrap_mmap __wrap_munmap __wrap_open __wrap_opendir __wrap_posix_memalign __wrap_powf __wrap_pread __wrap_printf __wrap_pthread_create __wrap_pthread_exit __wrap_pthread_getschedparam __wrap_pthread_getspecific __wrap_pthread_key_create __wrap_pthread_key_delete __wrap_pthread_setname_np __wrap_pthread_setschedparam __wrap_pthread_setspecific __wrap_read __wrap_readdir __wrap_readdir_r __wrap_realloc __wrap_realpath __wrap_recv __wrap_recvfrom __wrap_recvmsg __wrap_rename __wrap_rewinddir __wrap_rmdir __wrap_scandir __wrap_send __wrap_sendmsg __wrap_sendto __wrap_socket __wrap_stat __wrap_strdup __wrap_strndup __wrap_strtod __wrap_swprintf __wrap_syslog __wrap_unlink __wrap_valloc __wrap_vasprintf __wrap_wcstod __wrap_wcstol __wrap_write`) and 76 GL wrappers (`__wrap_gl*`, provided by libgles_apportable). These are all *bundled* so resolved inside the bundle; the corresponding real calls land in the system list above.

## 6. JNI surface

| Library | `JNI_OnLoad` | `Java_*` exports | Notes |
|---|---|---:|---|
| libApplication.so | yes | 90 | `MainThread.nativeRun`, `Lifecycle.*` (9), `VerdeActivity.nativeOn*` (+8 gpg), `GLSurfaceView.nativeInit/nativeOnSurfaceChanged`, `Window.nativeTouches*/nativeKey*`, `utils.RunLoop.nativeNext/nativeDispatchMessage`, ApportableIAP (6), PlusGamesManager (19), FlurryAdsShim (10), NoodleNews (4), HeyzapAds, C2DM, MotionManager (6), NoodleWebView, NoodlePermissionGranter |
| libBridgeKit.so | yes | 77 | UIKit-over-Android-widget bridge: `ui.View/TextField/TextView/ScrollView/AlertView/WebView/PickerView/...` callbacks, `GLSurfaceRenderer.surfaceCreated/surfaceChanged/drawFrame`, `GLSurfaceDispatch.AndroidGLSurfaceBindThread`, `KeyHandler`, `VerdeOutputStream`, `VerdeResponseHandler`, `LocationListener`, `MediaPlayer` |
| libOpenAL.so | yes (+`JNI_OnUnload`) | 0 | strings suggest `android/media/AudioTrack` through JNI as one backend, and OpenSL ES (dlopen) as another |
| libv.so | yes | 0 | `JNI_OnLoad` likely caches JavaVM and Build/Version info (inferred from strings) (`android/os/Build`, `Build$VERSION`) |
| others | no | 0 | |

Native -> Java calls: strings in libBridgeKit name **~51 `com.apportable.*` Java classes** looked up with `FindClass`/`GetMethodID` (ui widgets, GLSurface*, media, http, account, SocialShareManager, multicast, LocalNotification, c2dm RemoteNotifications, ...) plus `android/view/View`, `android/media/AudioTrack`, `android/os/Build`. The JNI function table used is the standard `JNIEnv` vtable (we must provide a full ~230-slot `JNINativeInterface` (JNI 1.6) for ARM32 guest pointers). Detailed Java-side replacement analysis belongs to the JNI/Java report.

## 7. Summary of what the host must provide

| Layer | Provide | Approx. size |
|---|---|---|
| bionic libc subset | 273 functions + 31 bionic-specific/ARM helpers | 304 symbols |
| libm | 68 functions (all single/double basic) | trivial: map to host libm |
| libdl | dlopen/dlsym/dlclose/dlerror/dladdr (must resolve against our loader, incl. `libOpenSLES.so`, `libEGL`, `libGLESv*`, `/system/bin/linker` string refs) | 5 |
| liblog | 3 | trivial |
| libjnigraphics | 3 (`AndroidBitmap_getInfo/lockPixels/unlockPixels`, used only by libBridgeKit with `jobject` Bitmaps) | trivial given a fake Bitmap |
| EGL | 1 (`eglGetCurrentContext`) + Java-side surface lifecycle | trivial |
| GLES1+2 | 380 entry points, stub the QCOM/AMD/NV/APPL/IMG extras | map to desktop GL/ANGLE/gl4es |
| OpenSL ES | `slCreateEngine` + 5 IIDs (engine, play, bufferqueue, androidsimplebufferqueue, record) via dlopen | implement an SL shim or make OpenAL-soft pick its non-SLES backend (patch `alc_opensles_probe`) |
| JNI | `JNIEnv`/`JavaVM` tables + ~167 callbacks to drive + ~51 Java classes to emulate | see Java report |
