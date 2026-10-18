# `bhpc-run --apk X` (play mode): bring-up notes

Build and run (needs the system libSDL2; `SDL_VIDEODRIVER=offscreen` or `xvfb-run` works headless):

```
cargo build --release -p bhpc-run --features play
SDL_VIDEODRIVER=offscreen target/release/bhpc-run --apk "$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" \
    --seconds 30 --shot /tmp/bhpc-frame.png --shot-at 300 [--click 512,552@20000]
```

Options: `--window WxH`, `--shot FILE` (+ `-first.png`), `--shot-at N` (swap number), `--exit-after-shot`, `--seconds N`,
`--click X,Y@MS`, `--alert-choice N`, `--check-gl`, `--trace-objc` (every Objective-C method-cache miss, a cheap
message trace), `--trace-jni --trace-jni-calls`, `--trace-fs` (also logs `dlopen`/`dlsym`), `--jni-report FILE`,
`--symbolize A,B`, `--data-dir D`. At the end the run prints where every guest thread last blocked (`[blocked]`), the
Java members that are still stubs and were called, GL calls that hit ignored stubs and the hottest host functions.

Result: 1.7.4, 1.7.5 and 1.7.6 boot to the main menu (about 900 frames in 35 s, input works: a click on "Create
World..." opens the world dialog).

## Threads

* host main thread: SDL window and event pump (`bhpc-app`), events go over a channel;
* "guest-main" host thread: the **guest main thread**. It opens the session, runs constructors (`libv_init` records it
  as `pthread_main_thread_np`, which is what `CFRunLoopGetMain` follows) and finally `MainThread.nativeRun` on a 32 MiB
  stack. It never returns;
* the Android **UI thread** is a guest thread (thunk start routine): Java start-up stages, then loop: pump the main
  looper, layout pass, alerts, input delivery. Lifecycle natives (`contextDidBecomeValid`, ...) only queue blocks on the
  main run loop, so they must come from a thread other than the main one and after the run loop exists is not required.

## What it took (in order)

* guest heap 64 MiB -> 736 MiB (libv allocates regions with `mmap`);
* `JNI_OnLoad`/`libv_init` ran on the loader thread, so the run loop `CFRunLoopGetMain` returned was never run: the
  game thread is now the guest main thread;
* fake Java: `com.apportable.ui.*` widgets (`create` returns an object whose `mObject` is the native pointer), a layout
  pass (`callLayoutSubviews(mObject)` for views marked dirty), `Window` focus after `makeKeyAndOrderFront`, `AlertView`
  (title/message/buttons collected, `nativeOnClick` + `nativeOnDismiss` when answered), `GLSurfaceView.nativeInit` and
  `nativeOnSurfaceChanged` stages, `AudioTrack` over the mixer, manifest booleans as environment
  (`apportable.opengles2`);
* the JNI allocator must hand out zeroed memory (lent UTF strings relied on it: `trueard/cache` instead of `true`);
* libdispatch needs real pipes/epoll (`vfd.rs`) and `RLIMIT_NOFILE` must not be unlimited (libkqueue sizes a table);
* LMDB needs `pread`/`pwrite` and coherent `mmap(MAP_SHARED)` (`mmapio.rs`);
* `dlopen`/`dlsym` over a snapshot of the module table plus the GL/EGL/OpenSL ES host libraries (`dl.rs`), including
  `RTLD_NEXT` (libgles wraps `eglSwapBuffers`);
* first launch shows a consent alert ("Enable multiplayer and free crystals?"); play mode presses the first button
  (`--alert-choice` overrides).

## Open issues / next blockers

* `sscanf`, `vsscanf`, `fscanf`, `strptime`, `dladdr`, `__futex_wait/wake`, `__cxa_begin_cleanup`, `__cxa_type_match`
  and `AndroidBitmap_*` are still bound to traps (none was hit in the first minute of the menu);
* networking is absent (`socket` fails with `EAFNOSUPPORT`): GDPR check, news, multiplayer;
* Java stubs still called: `Notifications.*`, `NoodleNewsClient.*`, `ApportableIAP.*`, `AndroidAudioManager.getInstance`,
  Flurry/Heyzap shims (all harmless so far);
* music (`MediaPlayer`), the on-screen keyboard (`TextField`/`TextView` focus callbacks into `InputMapper`), gamepads,
  fullscreen toggling and DPI scaling of the content size are not done; the SLES pump thread calls guest code without
  holding the scheduler baton.
