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
  the SLES pump thread calls guest code without
  holding the scheduler baton.

## Window size, input and scheduling fixes (click freeze, window scaling)

* **Render size.** The game announces its own size with `VerdeActivity.setContentSize` (second call wins; 1024x768
  for a 1280x960 screen, 1334x750 for 1600x900, 2048x1536 stays) like an Android fixed-size surface. The window
  keeps an offscreen framebuffer of that size (`bhpc-app/src/vsurface.rs`, behind `GlWindow::default_framebuffer`:
  `glBindFramebuffer(0)` binds it, `glGetIntegerv(FRAMEBUFFER_BINDING)` still answers 0) and draws it letterboxed
  into the drawable right before the swap (`compose`). `eglQuerySurface` reports the content size. Screenshots read
  the composed window.
* **Screen size.** `getScreenWidth/Height` and the display metrics are the drawable size and follow `Resized`
  (`Vm::set_screen_size`, then `SurfaceChanged`); the Java views are laid out in screen pixels, the game's touches
  are in content pixels: `Vm::content_scale` converts, `bhpc_input::Geometry` maps window pixels to content pixels
  through the letterbox (mouse events are logical pixels, so high-DPI works).
* **Lifecycle.** Focus loss/gain send no lifecycle (the game pauses and shows its pause menu on
  `applicationWillResignActive`; compositors flip focus around clicks). Minimise/restore go to background/foreground.
* **Dispatch.** libSystem's libkqueue makes its user-event wake-up with `syscall(351)` (eventfd) and timers with
  `syscall(350/353/354)` (timerfd). They were `ENOSYS`, so the dispatch manager thread never woke, `dispatch_after`
  blocks never ran (the tutorial popup's dimmer stayed up and the game ignored every touch,
  `[[BlockBackground instance] isUserInteractionEnabled]`), and libv's `write` wrapper failed with EBADF. `vfd`
  now implements them (`vfd/timerfd.rs`); `mmapio`'s `write` watcher hands descriptors of the table back to it.
* **`dlsym(RTLD_NEXT)`** also finds symbols of libraries loaded later (CoreFoundation links `_CFSocketStreamCreatePair`
  from CFNetwork this way). With that and the dispatch fix the start-up mission-list request no longer logs
  "Bad file descriptor".
* Scripts gained `focus on|off`, `minimize`, `restore`, `resize W H` and `key f11`.
* **Java views.** The popups/dialogs of `com.apportable.ui` (tutorial "WELCOME", ...) are drawn over the frame:
  `bhpc-jni/src/render.rs` rasterises the view tree (backgrounds, `ImageView`/`Button` bitmaps, label text) into a
  window-sized RGBA frame, `play.rs` passes it to the window (`GlWindow::set_overlay`) and `vsurface.rs` blends it in
  `compose`. See the gotchas section of `CLAUDE.md`.

## Wheel zoom in a world

* **Why the wheel pinch often did nothing.** The game eases `GameView.pinchScale` to one of its own levels
  (0.5, 1, 1.5, 2, 3, 8; switch points 0.6, 1.25, 1.75, 2.5, 5) whenever no pinch is in progress
  (`-[GameView update:accurateDT:]`). One wheel notch was a two-finger pinch of factor 1.2 that ended 150 ms later,
  short of the next switch point, so the zoom sprang back; only fast scrolling got past it.
* **Now.** In a world (`GameUi::zoom` readable) the mapper sends `HostAction::WheelZoom` (whole notches; touchpad
  fractions add up, a reversal starts afresh) instead of touches. `play.rs` steps one level per notch through the
  game's own `-[GameView pinchGesture:velocity:state:center:numberOfTouches:]` (began, changed for ~140 ms eased,
  ended; centre = cursor in view points from `GameView.windowInfo`, velocity NaN so no momentum), which sets the zoom
  and moves the camera around the centre like a finger pinch. Notches during a step re-aim the running pinch. If the
  game ignores the pinch (a button under the cursor), the zoom is written directly. Menus keep the touch pinch.
  `BHPC_TRACE_ZOOM=1` logs each step; scripts have `wheel DY X Y`.
* The controller's L1/R1 (one level out/in) and Y (cycle 1, 2, 3, 0.5) take the same path, centred on the
  controller cursor (the middle of the picture while it is hidden).
* Before a blockhead is warped in, the camera stays on the portal, so the zoom happens around the screen centre there.
