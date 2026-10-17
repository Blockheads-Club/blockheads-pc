# bhpc-input

Pure mapping layer (PLAN 1.7): host mouse, keyboard and window events in, the events the Blockheads
guest expects out. No SDL, no JNI, no guest memory; fully unit-tested (100 % line and function coverage).

```rust
let mut mapper = InputMapper::new(Config::default());      // 1024x768 content space
mapper.handle(HostEvent::MouseButton { .. }, now_ms, &mut sink);
mapper.tick(now_ms, &mut sink);                              // once per frame: ends idle wheel pinches
```

`sink` implements `EventSink`. Every `GuestEvent` knows its Java native (`GuestEvent::targets()` gives class,
method and JNI signature), so the sink is a thin decoder to JNI arguments.

## Mapping

See the table in `src/lib.rs`. Decisions beyond PLAN 1.7:

* Only one gesture at a time; extra buttons pressed meanwhile (and their releases) are ignored.
* Right button = two-finger **pan** (as in PLAN), not a long press. The game's long press is a held left button.
* Wheel pinch: pointers 0 and 1 start `pinch_start_radius` either side of the cursor, the radius changes by
  `pinch_step` per notch (positive `dy` = spread = zoom in) and the pinch ends `pinch_idle_ms` after the last
  notch (needs `tick`). A click ends it immediately.
* Ctrl + left drag = pinch around the press point; radius = start + dx - dy, clamped.
* Focus lost / minimise cancels the active gesture (`TouchCancel`) before the lifecycle events.
* While a guest text field has focus (`set_text_focus`), printable text arrives via `HostEvent::TextInput` and
  becomes `shouldChangeCharactersInRange` (UTF-16 `NSRange` units, caret tracked on the host) followed by
  `textDidChange` if the guest returns true; Backspace/Delete/arrows/Home/End edit, Enter is `shouldReturn`.
  Esc stays Back. Other keys go to `Window.nativeKeyDown/Up` with Android key codes.

## Wiring needed in bhpc-run / bhpc-jni

1. Implement `EventSink` over the fake JVM: decode `GuestEvent` by `targets()` and call the guest
   `Java_com_apportable_ui_Window_nativeTouchesBegin` etc. (static, JNIEnv + jclass first). `TextField.*` natives are
   instance methods whose first int is the field handle. Return the Java `boolean` from `deliver`.
2. The trailing `jlong` of the `Window` natives (`IFFJ`, `[I[F[FJ`, `IIJI`) is passed as `time_ms`; it is believed to be
   the event time. Verify against the first real call.
3. `SurfaceChanged` fans out to `GLSurfaceView.nativeOnSurfaceChanged(II)V` and
   `VerdeActivity.nativeRootGeometryChanged()V`; the first one's actual JNI shape must be confirmed in `bhpc-jni`.
4. Translate SDL events to `HostEvent` in `bhpc-app` (SDL_MOUSE*, SDL_KEYDOWN/UP, SDL_TEXTINPUT, SDL_WINDOWEVENT_*),
   call `SDL_StartTextInput/StopTextInput` on `HostAction::{Start,Stop}TextInput`, and `set_text_focus` when the
   guest's text field becomes/stops being first responder (`TextField.didBeginEditing`/`didEndEditing` come from
   the guest through JNI).
5. Wake the guest run loop after delivering input (PLAN: `RunLoop.nativeNext` wakes on SDL-driven injection).

Not covered: accelerometer (`MotionManager`), gamepads, IME composition.
