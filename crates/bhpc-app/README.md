# bhpc-app

Window and main-loop skeleton (PLAN 1.5). GPL-3.0-only.

* `App<B: Backend, C: Clock>`: event pump, fullscreen toggle (F11, Alt+Enter, consumed), vsync or `fallback_fps`
  pacing (`FramePacer`: no drift, no catch-up bursts), `Handler::event` / `Handler::render_frame` hooks.
* Two modes via `AppOptions::render_in_loop`:
  * `true`: this thread renders (`render_frame`, then `Backend::present`). Used for tests and first-frame work.
  * `false`: event pump only. The guest's GLThread renders through `bhpc-gles` and swaps with
    `eglSwapBuffers`; `ForwardHandler` sends `Event`s over an `mpsc` channel to the guest input thread.
* `Metrics { logical, drawable }`: window size, framebuffer size, `scale()` (DPI), coordinate conversion. Events carry
  logical window coordinates; `Event::Resized(Metrics)` fires on size and DPI changes.
* `sdl` feature (`sdl-bundled` builds SDL2 from source and links it statically): `sdl::SdlBackend::new` creates a GLES
  2.0 window (RGBA8, D24S8, resizable, high DPI) and returns `Arc<SdlGl>`, the `bhpc_gles::GlWindow` used by EGL
  (`SDL_GL_CreateContext` with `SHARE_WITH_CURRENT_CONTEXT` for shared contexts, `MakeCurrent`, `SwapWindow`,
  `SetSwapInterval`, `GetProcAddress`). `sdl::translate` maps SDL events (mouse, wheel, keys as HID scancodes, text,
  touch, focus, resize, quit) to `Event`.

## Wiring in `bhpc-run`

```rust
let (backend, gl) = SdlBackend::new(&WindowConfig::default())?;          // main thread
let (tx, rx) = std::sync::mpsc::channel();                                // rx -> guest input thread (UIKit touches)
std::thread::spawn(move || { /* build Gles with `gl` (see bhpc-gles README), run the guest */ });
let mut app = App::new(backend, SystemClock::new(), AppOptions { render_in_loop: false, ..Default::default() });
app.run(&mut ForwardHandler::new(tx));                                    // returns when the window closes
```

Drop the `App` (and with it the window) only after the guest threads stopped: `SdlGl` calls fail softly afterwards.

## Tests

Default build is SDL-free: `COVERAGE_PACKAGES=bhpc-app scripts/coverage.sh` (100 % lines and functions) uses a fake
backend and a fake clock. With the `sdl` feature, `translate` is unit tested and one `#[ignore]` test opens a real
window and renders through the full stack (JIT -> thunk -> marshalling -> driver, `glClear` + `glReadPixels`):

```
SDL_VIDEODRIVER=offscreen cargo test -p bhpc-app --features sdl -- --ignored   # NVIDIA EGL, passes
xvfb-run -a env SDL_VIDEODRIVER=x11 cargo test -p bhpc-app --features sdl -- --ignored   # Mesa llvmpipe, passes
```
