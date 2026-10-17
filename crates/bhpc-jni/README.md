# bhpc-jni

Fake `JavaVM` / `JNIEnv` for the Blockheads guest libraries (PLAN 1.4). The Android game is a thin
Java shell (`VerdeActivity`, `MainThread`, Looper, Apportable's `LibraryLoader`) around native code.
This crate is that shell on the PC: no JVM, no dex, no Java bytecode. License: GPL-3.0-only.

* **Object model** (`Vm`): classes, objects, strings (UTF-16), primitive and object arrays, boxes,
  local/global/weak references with frames, pending exceptions per guest thread, a mark-and-sweep
  `Vm::gc()` for callers that can guarantee no unreferenced `Obj` is held.
* **JNI tables** (`Vm::install`): the 233-slot `JNINativeInterface` and the `JNIInvokeInterface` as
  shared (re-entrant) thunks named `JNI:<Function>`; `JNIEnv*` is per guest thread
  (`Cpu::processor_id`). All `Call*/CallNonvirtual*/CallStatic*` in `...`, `V` (va_list) and `A`
  (jvalue[]) form, field accessors, strings, arrays, `RegisterNatives`, direct buffers.
* **Dispatch by class + name + signature**: `Vm::method_id`, `Vm::invoke`, `call_method`,
  `call_static`, `new_object`. Members are host closures (`ClassBuilder::method`), guest natives
  (`RegisterNatives` or a `Java_*` export found through `set_symbol_resolver`) or reported stubs.
* **Fake library** (`jlib`): `java.lang/util/io`, `android.os/content/res/util/view/provider/media/net`,
  `com.apportable.*` with PC values from `Platform` (screen size, density, locale, time zone,
  package, directories, android id). Assets come through the `Assets` trait.
* **Looper** (`looper`): `Looper/Handler/Message/MessageQueue` with timed messages. `Looper.loop()`
  returns at once: the host owns the loop (`Vm::pump`, `next_wake`).
* **Start-up** (`startup`): `native_env`, `Vm::call_onload`, `set_native_env`, `run_stage(Stage)`.

Unknown classes/members are *invented and reported* (`VmConfig::lenient`, default on) so boot is
iterated from `Vm::report()`: every looked-up member with `host | native | STUB` and call counts.
With `lenient = false` they raise `NoClassDefFoundError`/`NoSuchMethodError` like a real VM.

See `docs/m2/jni-surface.md` for the enumerated Java surface and what is implemented.

## Integration

`bhpc-run` uses `bhpc_jni::install(cpu, mem, alloc, cfg)` (module `host`): it builds the VM with the built-in library, registers the tables
and returns a `Jni` handle (`java_vm`, `usage()` summary, `enter`/`leave` frames around host-initiated calls such as `JNI_OnLoad`).
The allocator is the bionic heap, assets are `bhpc-vfs`' `AssetManager`, `Platform::for_guest` takes the Android paths from `GuestPaths`.
The longer sequence below is what the later stages (start-up natives, main loop) will add.

### Full sequence

```rust
use bhpc_jni::{native_env, new_vm, Assets, GfxHooks, Platform, Stage, VmConfig};

// 1. A guest heap for the VM's own buffers (strings lent to native code, JNIEnv structs, tables).
let jheap = GuestHeap::new(mem.clone(), JNI_HEAP_BASE, 32 << 20)?;   // or an adapter over the bionic heap
// 2. Platform values + assets (the VFS crate implements `Assets`) + optional GL hooks.
let mut cfg = VmConfig::new(Platform { apk_path, files_dir, ..Platform::default() }, Box::new(vfs));
cfg.gfx = Arc::new(gl_hooks);                       // optional: swapBuffers / contexts / content size
let vm = new_vm(mem.clone(), Box::new(jheap), cfg);
// 3. Install the tables on the CPU whose thunk table all threads share (before spawning threads).
let layout = vm.install(&cpu)?;                      // layout.java_vm is the JavaVM*
// 4. Let `Java_*` exports of every loaded module bind lazily to the fake classes.
let exports: HashMap<String, u32> = linker.modules().iter()
    .flat_map(|m| m.exports.iter().filter(|(n, _)| n.starts_with("Java_")).map(|(n, a)| (n.clone(), *a))).collect();
vm.set_symbol_resolver(move |n| exports.get(n).copied());
// 5. After the constructors ran: JNI_OnLoad of every module that has one, in load order.
for m in linker.modules() { if let Some(a) = m.export("JNI_OnLoad") { vm.call_onload(&mut cpu, a)?; } }
// 6. The Java start-up sequence, minus Java.
vm.run_stage(&mut cpu, Stage::LibraryInit)?;
vm.set_native_env(&mut cpu, &native_env(&platform))?;
vm.attach_main_looper(cpu.processor_id());           // the thread that will run MainThreadRun
for s in [Stage::ActivityCreate, Stage::ActivityStart, Stage::ActivityResume, Stage::RootGeometry] {
    vm.run_stage(&mut cpu, s)?;
}
// ... create the GL context, then Stage::ContextValid, Stage::WillEnterForeground, Stage::DidBecomeActive,
// then Stage::MainThreadRun on a big-stack guest thread (it does not return).
```

* `bhpc-run`/bionic wiring needed outside this crate: none of this crate's functions are called
  by the libc layer; `Vm` is `Send + Sync` and clones are handles. Call `vm.install` **once** per
  guest memory (thunk names are `JNI:*`); extra threads made with `Cpu::new_thread` share the table
  and get their own `JNIEnv*` automatically.
* The main loop of the app (`MainThread`) is the host's: while the guest runs, another host thread
  or the thunk glue may call `vm.post_main(runnable, delay)` and the guest side drains
  `Handler`/`Looper` through `RunLoop.nativeNext` (a native of the game, not ours). `vm.pump`,
  `vm.next_due`, `vm.next_wake` are for a host loop that runs `Runnable`s itself.
* Touch/key/surface events are *Java -> native* calls: use `vm.call_static` / `vm.call_method` with
  the exact descriptors from `docs/feasibility/B-jni-java-surface.md` section 3
  (`com/apportable/ui/Window.nativeTouchesBegin`, `GLSurfaceRenderer.drawFrame`, ...). The `jlong`
  native-object pointers are whatever the guest handed to its Java constructor, which the Java-side
  fakes will have to remember once the bridge is observed running.

## Not implemented (yet)

HTTP (`HttpClient` family), `MediaPlayer`/`AudioTrack` objects, `ui.*` widgets, `Intent`,
`SharedPreferences` persistence (in memory only), `java.util.Date/Calendar`, reflection
(`ToReflected*` return null), `DefineClass` (DexMaker proxies) and weak-reference clearing.
Each of these shows up as a `STUB` in `Vm::report()` the first time the game touches it.
