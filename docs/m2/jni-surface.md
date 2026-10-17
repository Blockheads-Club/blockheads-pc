# Fake JVM: the Java surface (M2, `bhpc-jni`)

How the surface was enumerated, what the libraries do at `JNI_OnLoad`, and what the fake class
library implements. No game code, dex or string dumps are stored; only names and descriptors that
are needed to implement the interface.

## Method

1. **Static, per library:** `readelf --dyn-syms` for `JNI_OnLoad`/`Java_*` and the disassembly of each
   `JNI_OnLoad` (JNIEnv function-table offsets), from the APK's own libraries.
2. **Runtime:** `crates/bhpc-jni/tests/real_apk.rs` (ignored, needs `BHPC_APK`) loads all 25 libs,
   runs the constructors, calls every `JNI_OnLoad` and the start-up stages against the fake VM
   and prints `Vm::report()` (every looked-up class/member with its kind and call count).
3. **dex descriptors** of `com.apportable.*` (activity, MainThread, Lifecycle, RunLoop, ...) from the
   earlier analysis (`docs/feasibility/B-jni-java-surface.md`), used to give the fakes the exact
   descriptors the native side expects.

## What the libraries do at `JNI_OnLoad` (1.7.4, measured)

| Library | JNI usage |
|---|---|
| `libv.so` | `vm->GetEnv(0x10004)`; `FindClass("com/apportable/LibraryLoader")` and `RegisterNatives` of `debugWait()V`, `init()V`, `setenv(String,String)V`; `FindClass("com/apportable/Library")` and `RegisterNatives` of `resolveLinkages(String)String[]`. Returns `0x10004`. |
| `libBridgeKit.so` | `AndroidGetJavaEnv`, `NewGlobalRef`, `DeleteLocalRef`, `GetStaticMethodID`, `CallObjectMethod` on `Class.getClassLoader()`; holds `Class.forName(String,boolean,ClassLoader)` for later `FindClass` replacement. Returns `0x10004`. |
| `libOpenAL.so` | Caches the `JavaVM*` (and calls a function pointer in its own data first). |
| `libApplication.so` | `pthread_self` only. Its 90 `Java_*` exports are found by the symbol resolver. |

Everything else is *lazy*: BridgeKit's ObjC wrapper classes look their Java class/member up the
first time an ObjC class is used (`+initializeJava`), with names and descriptors built at run time.
That needs libSystem's ObjC runtime to be initialised, which the bring-up has not reached yet (63
constructors still fail in the libc layer), so the lazy part could not be traced at run time. For
that part the surface comes from the static analysis: ~70 wrapper classes referenced by
`libApplication`, ~498 registered class names, boot needs ~45 classes / 130-180 members.

## Implemented (host behaviour)

| Area | Classes |
|---|---|
| `java.lang` | `Object`, `Class` (forName, getName, getSimpleName, getClassLoader, isInstance, getSuperclass), `ClassLoader` (loadClass), `String` (25 methods), `StringBuilder/StringBuffer`, boxes + `Number`, `System` (time, getenv, getProperty, arraycopy, exit, identityHashCode), `Thread` (currentThread, names, ids), `Throwable` and 30 exception/error classes |
| `java.util` | `ArrayList/LinkedList/Vector/LinkedHashSet/HashSet` (list ops, iterator), `HashMap/LinkedHashMap/TreeMap` (insertion ordered), `Locale`, `TimeZone`, `UUID` |
| `java.io` | `File` (real file system), `InputStream`/`ByteArrayInputStream`/`FileInputStream` |
| `android.os` | `Build`, `Build$VERSION`, `Bundle`, `Environment`, `Process`, `SystemClock`, `Looper`, `Handler`, `Message`, `MessageQueue` |
| `android.content` | `Context`/`ContextWrapper`/`Application`/`Activity` (files/cache/obb/external dirs, package paths, assets, resources, system services, `SharedPreferences` + `Editor`), `PackageManager` (`hasSystemFeature`, `getPackageInfo`), `res.Resources/Configuration/AssetManager` |
| `android.util/view/...` | `DisplayMetrics`, `Log` (to the VM log sink), `Display`, `WindowManager`, `Point`, `Settings$Secure` (android id), `AudioManager`, `ConnectivityManager`/`NetworkInfo` (always connected, Wi-Fi) |
| `com.apportable.*` | `activity.VerdeActivity` (screen/content sizes, EGL context callbacks through `GfxHooks`, `queueEvent/runOnThread`, `uuid`, `androidId`, `hasSystemFeature`, `terminateProcess`, ...), `MainThread` (handler, thread, tick actions), `utils.RunLoop`, `Lifecycle`, `LibraryLoader`, `Library`, `ui.Device`, `app.VerdeApplication` |

PC values come from `Platform` (defaults: 720x1280, 320 dpi, `en_US`, UTC, `com.majicjungle.blockheads`,
Android 4.4.4 / SDK 19 `bhpc` device, directories under `~/.local/share/blockheads-pc`).

## Declared natives (bound by `RegisterNatives` or `Java_*` exports)

`LibraryLoader` (3), `Library.resolveLinkages`, `Lifecycle` (8), `VerdeActivity` (21 of its 23),
`MainThread.nativeRun`, `RunLoop.nativeNext`, `ui.Device.nativeOnOrientationChanged`. Any other
`Java_*` export is bound lazily on first call through the symbol resolver.

## Policy for everything else

`VmConfig::lenient` (default): an unknown class is created empty, an unknown method/field is
created as a stub that returns zero/null/false; each is logged once and appears in
`Vm::report()` with `STUB` and its call count. This matches the "dummy id for any member that is
never called" boot strategy of the earlier analysis. Strict mode raises the Java errors instead.

## Known gaps / risks

* The lazy BridgeKit lookups (ObjC wrapper -> Java) are unobserved; expect a round of stub
  filling once libSystem initialises (`vm.report().stubs()` is the work list).
* BridgeKit builds Java proxies with DexMaker (`DefineClass`, `com.google.dexmaker.*`). `DefineClass`
  fails with `NoClassDefFoundError`; the five call sites need host-side proxy classes or a patch.
* Threads: one fake `Thread` object per `Cpu::processor_id`; `Looper.loop()` does not block.
* `SharedPreferences` is in memory only (no persistence), `File` is the real file system.
* `Vm::gc()` is explicit; nothing collects automatically.
