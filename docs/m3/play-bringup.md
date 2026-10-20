# `bhpc-run --apk X` (play mode): bring-up notes

Build and run (needs the system libSDL2; `SDL_VIDEODRIVER=offscreen` or `xvfb-run` works headless):

```
cargo build --release -p bhpc-run --features play
SDL_VIDEODRIVER=offscreen target/release/bhpc-run --apk "$HOME/Downloads/The Blockheads_1.7.4_APKPure.apk" \
    --seconds 30 --shot /tmp/bhpc-frame.png --shot-at 300 [--click 512,552@20000]
```

Options: `--window WxH`, `--shot FILE` (+ `-first.png`), `--shot-at N` (swap number), `--exit-after-shot`, `--seconds N`,
`--click X,Y@MS`, `--auto-alerts`, `--alert-choice N` (implies `--auto-alerts`), `--check-gl`, `--trace-objc` (every Objective-C method-cache miss, a cheap
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
* first launch shows a consent alert ("Enable multiplayer and free crystals?"); it is drawn as a host dialog
  (`render/dialog.rs`) like every alert; `--auto-alerts` presses the first button (`--alert-choice N` another one).

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

## Links, mail, clipboard, photos and join links (desktop integration)

The fake Java layer (`bhpc-jni/src/jlib/desktop.rs`) gives the game a working `Uri`/`Intent` model and hands what it
asks for to `bhpc_jni::SystemHooks`, which `bhpc-run/src/desktop.rs` implements:

- Links (More Games, the pause-menu links, Free Offers, support): `openURL:` -> `Intent(ACTION_VIEW, uri)` ->
  `resolveActivity` (answered for `http`, `https`, `mailto`, `market://`, the latter mapped to the Play Store web
  page) -> `startActivity` -> the opener. Other schemes are logged and dropped.
- Mail (`VerdeActivity.sendEmail`, `canSendEmail` is now true): a `mailto:` link with subject, body, to/cc/bcc
  (an HTML body is reduced to text; attachments are dropped). A `SEND` intent with recipients or `message/rfc822`
  becomes a `mailto:` link too.
- Sharing (`SocialShareManager`, other `SEND` intents): the text and links go to the clipboard and
  "COPIED TO CLIPBOARD" shows over the game. Pictures are not shared. `canSendFacebook`/`canSendTweet` stay false.
- Clipboard (`ClipboardManager`/`ClipData`): in a window of our own SDL's clipboard (set on the main thread); in the
  launcher's window the runner prints `@copy <text, %XX-escaped>` on stdout and the launcher sets the GTK clipboard.
  Reading answers the last copied text (the host clipboard is not read from guest threads).
- Camera photos (`UIImageWriteToSavedPhotosAlbum` writes `<public dir>/The Blockheads/<time>.png` below
  `ANDROID_EXTERNAL_STORAGE`): `/sdcard/Pictures` and `/sdcard/DCIM` are mounted on the user's Pictures folder
  (`XDG_PICTURES_DIR`, else the platform's, else `~/Pictures`). `MediaScannerConnection.scanFile` shows
  "PHOTO SAVED: <host path>".
- Help/Credits (`NoodleWebView.showBlockheadsWebView(html, editable, id)`, also custom-rules help): the HTML and the
  assets it refers to (`GameResources/instructions/...`, style sheet pictures) are written to
  `$TMPDIR/blockheads-pc-pages/<pid>-<n>/index.html` and opened in the browser. The *editable* page (the server
  welcome message, `editable = true`) opens read-only: editing needs an in-game text editor that calls the native
  `BlockheadsWebView.nativeSetWelcomeMessage(String, long id)` with the edited text (not done).

Opener: `BHPC_OPENER` (a command; `%s` is the target, else it is appended), else `BROWSER` (first entry), else
`xdg-open` / `open` / `rundll32 url.dll,FileProtocolHandler`. Headless runs and tests set `BROWSER=true` (or a
recording `BHPC_OPENER`) so nothing opens on the desktop.

Join links: `bhpc-run --open-uri URL` calls `VerdeActivity.nativeHandleUri(URL)` after the start-up lifecycle, as
Android does with the data of the launching intent. The launcher takes `--open-uri URL` (or a bare `http(s)://`
or `blockheads://` argument) and passes it to the next game start. The game builds invite links as
`http://theblockheads.net/join.php?ip=%@&port=%@&name=%@` (or `?id=%@&name=%@`). Its Android manifest only claims
`http://theblockheads.net/...`, `http://blockheads.noodlecake.com/...` and a malformed scheme
`theblockheads.net/join`; the binary also has a `blockheads://` prefix next to `http://` (probably the iOS scheme),
but whether the link handler acts on either form is unverified (a fresh data dir goes straight into the first world,
where the call had no visible effect). So no scheme is registered in the bundle's `.desktop` file yet; to try it:
`xdg-mime default blockheads-pc.desktop x-scheme-handler/blockheads` with `Exec=blockheads-pc %u`.

## Store, achievements and "Your pic"

* **Store (in-app billing).** `ApportableIAP` answers like a device without Google Play billing: `setupFinished`
  reaches the game after start-up, product queries fail at once (`querySkusResponse(.., error)`), purchases and
  restores answer false. The game has no "store unavailable" screen of its own: its prices stay `...`, a pack cannot
  be bought (no product), Restore ends in the game's own failure message. Nothing is ever bought.
* **Achievements (Google Play Games).** The start-up sign-in fails with `SERVICE_MISSING` (`nativeOnConnectionFailed`);
  "View achievements" then only asks `isLoggedIn` (the game signs in once per run) and the host shows
  "ACHIEVEMENTS ARE NOT AVAILABLE ON PC". The return to the main menu asks `isLoggedIn` too but queries the products
  right after it, which keeps it quiet (`bhpc-jni/src/jlib/services.rs`).
* **"Your pic".** `startActivityForResult(ACTION_PICK, content://media/internal/images/media)` (request 9001) opens a
  file dialog (`bhpc-run/src/picker.rs`: zenity or kdialog, PowerShell on Windows, osascript on macOS) on a thread;
  the answer is `nativeOnActivityResult(9001, RESULT_OK, intent with file://...)` or `RESULT_CANCELED`, and
  `MediaStore.Images.Media.getBitmap` decodes the PNG/JPEG (scaled to 1024 px at most). `BHPC_PICK_IMAGE=FILE`
  replaces the dialog (headless runs; empty = cancel).
