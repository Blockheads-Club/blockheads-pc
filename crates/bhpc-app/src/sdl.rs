//! SDL2 backend: a GLES 2.0 window, event translation and the [`bhpc_gles::GlWindow`] implementation.
//!
//! Threads: SDL's event pump and window calls stay on the thread that created [`SdlBackend`] (the host main
//! thread). [`SdlGl`] only uses `SDL_GL_*` calls, which SDL allows from any thread (a context must be current on
//! one thread at a time; `bhpc-gles` makes contexts current in the calling guest thread only). The backend must
//! outlive every use of [`SdlGl`]: dropping it destroys the window, after which [`SdlGl`] methods fail softly.

use crate::app::Backend;
use crate::event::{Event, Key, Mods, MouseButton, TouchPhase};
use crate::metrics::Metrics;
use crate::WindowConfig;
use bhpc_gles::{GlWindow, HostContext};
use sdl2::event::{Event as Sdl, WindowEvent};
use sdl2::keyboard::Mod;
use sdl2::sys;
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Convert an SDL event. `metrics` is the current window metrics (for touch coordinates and resizes).
pub fn translate(ev: Sdl, metrics: Metrics) -> Option<Event> {
    Some(match ev {
        Sdl::Quit { .. } => Event::Quit,
        Sdl::Window { win_event, .. } => match win_event {
            WindowEvent::SizeChanged(..) | WindowEvent::Resized(..) => Event::Resized(metrics),
            WindowEvent::FocusGained => Event::Focus(true),
            WindowEvent::FocusLost => Event::Focus(false),
            WindowEvent::Minimized => Event::Visible(false),
            WindowEvent::Restored => Event::Visible(true),
            _ => return None,
        },
        Sdl::MouseMotion { x, y, .. } => Event::MouseMove { x: x as f32, y: y as f32 },
        Sdl::MouseButtonDown { mouse_btn, x, y, .. } => {
            Event::MouseButton { button: button(mouse_btn), pressed: true, x: x as f32, y: y as f32 }
        }
        Sdl::MouseButtonUp { mouse_btn, x, y, .. } => {
            Event::MouseButton { button: button(mouse_btn), pressed: false, x: x as f32, y: y as f32 }
        }
        Sdl::MouseWheel { precise_x, precise_y, .. } => Event::Wheel { dx: precise_x, dy: precise_y },
        Sdl::KeyDown { scancode: Some(sc), repeat, keymod, .. } => {
            Event::Key { key: Key(sc as u32), pressed: true, repeat, mods: mods(keymod) }
        }
        Sdl::KeyUp { scancode: Some(sc), keymod, .. } => {
            Event::Key { key: Key(sc as u32), pressed: false, repeat: false, mods: mods(keymod) }
        }
        Sdl::TextInput { text, .. } => Event::Text(text),
        Sdl::FingerDown { finger_id, x, y, .. } => touch(finger_id, TouchPhase::Down, x, y, metrics),
        Sdl::FingerMotion { finger_id, x, y, .. } => touch(finger_id, TouchPhase::Move, x, y, metrics),
        Sdl::FingerUp { finger_id, x, y, .. } => touch(finger_id, TouchPhase::Up, x, y, metrics),
        _ => return None,
    })
}

fn touch(id: i64, phase: TouchPhase, x: f32, y: f32, m: Metrics) -> Event {
    Event::Touch { id, phase, x: x * m.logical.0 as f32, y: y * m.logical.1 as f32 }
}

fn button(b: sdl2::mouse::MouseButton) -> MouseButton {
    use sdl2::mouse::MouseButton as B;
    match b {
        B::Left => MouseButton::Left,
        B::Middle => MouseButton::Middle,
        B::Right => MouseButton::Right,
        B::X1 => MouseButton::Other(4),
        B::X2 => MouseButton::Other(5),
        B::Unknown => MouseButton::Other(0),
    }
}

fn mods(m: Mod) -> Mods {
    Mods {
        shift: m.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD),
        ctrl: m.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD),
        alt: m.intersects(Mod::LALTMOD | Mod::RALTMOD),
    }
}

/// The SDL window, event pump and main-thread side of the GL window.
pub struct SdlBackend {
    _sdl: sdl2::Sdl,
    audio: Option<sdl2::audio::AudioDevice<AudioOut>>,
    video: sdl2::VideoSubsystem,
    window: sdl2::video::Window,
    pump: sdl2::EventPump,
    alive: Arc<AtomicBool>,
}

impl Drop for SdlBackend {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

/// Fills interleaved stereo `i16` frames.
pub type AudioFill = Box<dyn FnMut(&mut [i16]) + Send>;

/// Audio callback of the SDL device.
pub struct AudioOut(AudioFill);

impl sdl2::audio::AudioCallback for AudioOut {
    type Channel = i16;
    fn callback(&mut self, out: &mut [i16]) {
        (self.0)(out);
    }
}

/// The GL half of the window, usable from guest threads.
pub struct SdlGl {
    window: *mut sys::SDL_Window,
    alive: Arc<AtomicBool>,
    /// False: every swap interval the game asks for becomes 0 (the vsync setting is off).
    vsync: AtomicBool,
    surface: crate::vsurface::VirtualSurface,
}

// SAFETY: only SDL_GL_* functions are called through the pointer, which SDL permits from any thread.
unsafe impl Send for SdlGl {}
unsafe impl Sync for SdlGl {}

impl SdlBackend {
    /// Create a window with a GLES 2.0 (RGBA8, 24-bit depth, 8-bit stencil) drawable.
    pub fn new(cfg: &WindowConfig) -> Result<(SdlBackend, Arc<SdlGl>), String> {
        let sdl = sdl2::init()?;
        let video = sdl.video()?;
        let attr = video.gl_attr();
        attr.set_context_profile(sdl2::video::GLProfile::GLES);
        attr.set_context_version(2, 0);
        attr.set_red_size(8);
        attr.set_green_size(8);
        attr.set_blue_size(8);
        attr.set_alpha_size(8);
        attr.set_depth_size(24);
        attr.set_stencil_size(8);
        attr.set_double_buffer(true);
        let mut b = video.window(&cfg.title, cfg.size.0, cfg.size.1);
        b.opengl();
        if cfg.resizable {
            b.resizable();
        }
        if cfg.high_dpi {
            b.allow_highdpi();
        }
        let window = b.build().map_err(|e| e.to_string())?;
        let pump = sdl.event_pump()?;
        video.text_input().start();
        let alive = Arc::new(AtomicBool::new(true));
        let gl = Arc::new(SdlGl {
            window: window.raw(),
            alive: alive.clone(),
            vsync: AtomicBool::new(true),
            surface: crate::vsurface::VirtualSurface::new(),
        });
        Ok((SdlBackend { _sdl: sdl, audio: None, video, window, pump, alive }, gl))
    }

    /// Open the default audio device (stereo `i16`, `rate` Hz) and start pulling samples from `fill`.
    pub fn open_audio(&mut self, rate: i32, fill: impl FnMut(&mut [i16]) + Send + 'static) -> Result<(), String> {
        let audio = self._sdl.audio()?;
        let spec = sdl2::audio::AudioSpecDesired { freq: Some(rate), channels: Some(2), samples: Some(1024) };
        let dev = audio.open_playback(None, &spec, |_| AudioOut(Box::new(fill)))?;
        dev.resume();
        self.audio = Some(dev);
        Ok(())
    }

    /// Set the window icon from RGBA pixels (`width` x `height`, 4 bytes each).
    pub fn set_icon(&mut self, rgba: &mut [u8], width: u32, height: u32) -> bool {
        let fmt = sdl2::pixels::PixelFormatEnum::ABGR8888;
        match sdl2::surface::Surface::from_data(rgba, width, height, width * 4, fmt) {
            Ok(icon) => {
                self.window.set_icon(icon);
                true
            }
            Err(_) => false,
        }
    }

    /// Resize the window (logical pixels); the window system answers with a `Resized` event.
    pub fn set_size(&mut self, width: u32, height: u32) -> bool {
        self.window.set_size(width, height).is_ok()
    }

    /// Name of the active video driver (for logs).
    pub fn driver(&self) -> &'static str {
        self.video.current_video_driver()
    }
}

impl Backend for SdlBackend {
    fn poll_event(&mut self, timeout: Duration) -> Option<Event> {
        let mut wait = timeout.as_millis() as u32;
        loop {
            let ev = if wait > 0 { self.pump.wait_event_timeout(wait) } else { self.pump.poll_event() }?;
            wait = 0;
            if let Some(e) = translate(ev, self.metrics()) {
                return Some(e);
            }
        }
    }

    fn metrics(&self) -> Metrics {
        Metrics::new(self.window.size(), self.window.drawable_size())
    }

    fn set_fullscreen(&mut self, on: bool) -> bool {
        let t = if on { sdl2::video::FullscreenType::Desktop } else { sdl2::video::FullscreenType::Off };
        self.window.set_fullscreen(t).is_ok()
    }

    fn present(&mut self) {
        // SAFETY: the window is alive as long as `self`.
        unsafe { sys::SDL_GL_SwapWindow(self.window.raw()) }
    }

    fn set_vsync(&mut self, on: bool) -> bool {
        // SAFETY: plain SDL call; fails (non-zero) without a current context.
        unsafe { sys::SDL_GL_SetSwapInterval(on as i32) == 0 }
    }

    fn set_title(&mut self, title: &str) {
        let _ = self.window.set_title(title);
    }
}

fn ctx_nonzero(ctx: usize) -> Option<usize> {
    (ctx != 0).then_some(ctx)
}

impl SdlGl {
    /// Whether the game may turn vsync on (the launcher's vsync setting). Applies to later swap-interval requests.
    pub fn set_vsync_allowed(&self, allowed: bool) {
        self.vsync.store(allowed, Ordering::SeqCst);
    }

    /// The window as `compose` would draw it, read back as RGBA (bottom row first) with its size; rendered into a
    /// texture of the drawable's size, so it is right even when the window's own surface is not (the offscreen video
    /// driver keeps the surface size it was created with). Needs the game's context to be current.
    pub fn compose_read(&self) -> Option<(u32, u32, Vec<u8>)> {
        if !self.live() {
            return None;
        }
        // SAFETY: plain SDL query.
        let ctx = ctx_nonzero(unsafe { sys::SDL_GL_GetCurrentContext() } as usize)?;
        let size = self.drawable_size();
        self.surface.compose_read(ctx, size, &|n| self.proc_address(n)).map(|px| (size.0, size.1, px))
    }

    fn live(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl GlWindow for SdlGl {
    fn create_context(&self, share: Option<HostContext>) -> Option<HostContext> {
        if !self.live() {
            return None;
        }
        // SAFETY: SDL_GL_* calls on a live window. Sharing works with the context current in this thread, so the
        // shared one is made current for the duration of the call and the previous one is restored.
        unsafe {
            let previous = sys::SDL_GL_GetCurrentContext();
            if let Some(s) = share {
                if sys::SDL_GL_MakeCurrent(self.window, s as usize as sys::SDL_GLContext) != 0 {
                    return None;
                }
                sys::SDL_GL_SetAttribute(sys::SDL_GLattr::SDL_GL_SHARE_WITH_CURRENT_CONTEXT, 1);
            }
            let ctx = sys::SDL_GL_CreateContext(self.window);
            sys::SDL_GL_SetAttribute(sys::SDL_GLattr::SDL_GL_SHARE_WITH_CURRENT_CONTEXT, 0);
            if share.is_some() || !previous.is_null() {
                // CreateContext made the new context current; put the old state back.
                sys::SDL_GL_MakeCurrent(self.window, previous);
            }
            (!ctx.is_null()).then_some(ctx as usize as HostContext)
        }
    }

    fn destroy_context(&self, ctx: HostContext) {
        if self.live() {
            // SAFETY: `ctx` came from `create_context`.
            unsafe { sys::SDL_GL_DeleteContext(ctx as usize as sys::SDL_GLContext) }
        }
    }

    fn make_current(&self, ctx: Option<HostContext>) -> bool {
        if !self.live() {
            return false;
        }
        let c = ctx.map_or(std::ptr::null_mut(), |c| c as usize as sys::SDL_GLContext);
        // SAFETY: valid window, context pointer from `create_context` or null.
        let ok = unsafe { sys::SDL_GL_MakeCurrent(self.window, c) == 0 };
        if ok && !c.is_null() && !self.vsync.load(Ordering::SeqCst) {
            // The EGL default is vsync on; the setting is off.
            // SAFETY: plain SDL call with the context just made current.
            unsafe { sys::SDL_GL_SetSwapInterval(0) };
        }
        ok
    }

    fn swap_buffers(&self) {
        if self.live() {
            self.compose();
            // SAFETY: live window.
            unsafe { sys::SDL_GL_SwapWindow(self.window) }
        }
    }

    fn set_surface_size(&self, width: u32, height: u32) {
        self.surface.set_size(width, height);
    }

    fn surface_size(&self) -> (u32, u32) {
        self.surface.size().unwrap_or_else(|| self.drawable_size())
    }

    fn default_framebuffer(&self) -> u32 {
        if !self.live() {
            return 0;
        }
        // SAFETY: plain SDL query.
        let ctx = unsafe { sys::SDL_GL_GetCurrentContext() } as usize;
        ctx_nonzero(ctx).map_or(0, |ctx| self.surface.framebuffer(ctx, &|n| self.proc_address(n)))
    }

    fn set_overlay(&self, overlay: Option<bhpc_gles::Overlay>) {
        self.surface.set_overlay(overlay);
    }

    fn compose(&self) {
        if !self.live() {
            return;
        }
        // SAFETY: plain SDL query.
        let ctx = unsafe { sys::SDL_GL_GetCurrentContext() } as usize;
        if let Some(ctx) = ctx_nonzero(ctx) {
            self.surface.compose(ctx, self.drawable_size(), &|n| self.proc_address(n));
        }
    }

    fn set_swap_interval(&self, interval: i32) -> bool {
        let interval = if self.vsync.load(Ordering::SeqCst) { interval } else { 0 };
        // SAFETY: plain SDL call.
        self.live() && unsafe { sys::SDL_GL_SetSwapInterval(interval) == 0 }
    }

    fn drawable_size(&self) -> (u32, u32) {
        if !self.live() {
            return (0, 0);
        }
        let (mut w, mut h) = (0, 0);
        // SAFETY: live window, valid out pointers.
        unsafe { sys::SDL_GL_GetDrawableSize(self.window, &mut w, &mut h) };
        (w.max(0) as u32, h.max(0) as u32)
    }

    fn proc_address(&self, name: &str) -> *const c_void {
        let Ok(c) = CString::new(name) else { return std::ptr::null() };
        // SAFETY: valid NUL-terminated string; needs a current context on some platforms.
        unsafe { sys::SDL_GL_GetProcAddress(c.as_ptr()) as *const c_void }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl2::keyboard::Scancode;
    use sdl2::mouse::{MouseState, MouseWheelDirection};

    const M: Metrics = Metrics { logical: (800, 400), drawable: (1600, 800) };

    fn ev(e: Sdl) -> Option<Event> {
        translate(e, M)
    }

    #[test]
    fn events_are_translated() {
        assert_eq!(ev(Sdl::Quit { timestamp: 0 }), Some(Event::Quit));
        let win = |w| Sdl::Window { timestamp: 0, window_id: 1, win_event: w };
        assert_eq!(ev(win(WindowEvent::SizeChanged(1, 1))), Some(Event::Resized(M)));
        assert_eq!(ev(win(WindowEvent::Resized(1, 1))), Some(Event::Resized(M)));
        assert_eq!(ev(win(WindowEvent::FocusGained)), Some(Event::Focus(true)));
        assert_eq!(ev(win(WindowEvent::FocusLost)), Some(Event::Focus(false)));
        assert_eq!(ev(win(WindowEvent::Minimized)), Some(Event::Visible(false)));
        assert_eq!(ev(win(WindowEvent::Restored)), Some(Event::Visible(true)));
        assert_eq!(ev(win(WindowEvent::Enter)), None);
        let ms = MouseState::from_sdl_state(0);
        assert_eq!(
            ev(Sdl::MouseMotion { timestamp: 0, window_id: 1, which: 0, mousestate: ms, x: 3, y: 4, xrel: 0, yrel: 0 }),
            Some(Event::MouseMove { x: 3.0, y: 4.0 })
        );
        for (b, want) in [
            (sdl2::mouse::MouseButton::Left, MouseButton::Left),
            (sdl2::mouse::MouseButton::Middle, MouseButton::Middle),
            (sdl2::mouse::MouseButton::Right, MouseButton::Right),
            (sdl2::mouse::MouseButton::X1, MouseButton::Other(4)),
            (sdl2::mouse::MouseButton::X2, MouseButton::Other(5)),
            (sdl2::mouse::MouseButton::Unknown, MouseButton::Other(0)),
        ] {
            let down =
                Sdl::MouseButtonDown { timestamp: 0, window_id: 1, which: 0, mouse_btn: b, clicks: 1, x: 1, y: 2 };
            let up = Sdl::MouseButtonUp { timestamp: 0, window_id: 1, which: 0, mouse_btn: b, clicks: 1, x: 1, y: 2 };
            assert_eq!(ev(down), Some(Event::MouseButton { button: want, pressed: true, x: 1.0, y: 2.0 }));
            assert_eq!(ev(up), Some(Event::MouseButton { button: want, pressed: false, x: 1.0, y: 2.0 }));
        }
        let wheel = Sdl::MouseWheel {
            timestamp: 0,
            window_id: 1,
            which: 0,
            x: 0,
            y: 1,
            direction: MouseWheelDirection::Normal,
            precise_x: 0.5,
            precise_y: -1.5,
            mouse_x: 0,
            mouse_y: 0,
        };
        assert_eq!(ev(wheel), Some(Event::Wheel { dx: 0.5, dy: -1.5 }));
        let keymod = Mod::LALTMOD | Mod::RCTRLMOD | Mod::LSHIFTMOD;
        let down = Sdl::KeyDown {
            timestamp: 0,
            window_id: 1,
            keycode: None,
            scancode: Some(Scancode::F11),
            keymod,
            repeat: true,
        };
        assert_eq!(
            ev(down),
            Some(Event::Key {
                key: Key::F11,
                pressed: true,
                repeat: true,
                mods: Mods { shift: true, ctrl: true, alt: true }
            })
        );
        let up = Sdl::KeyUp {
            timestamp: 0,
            window_id: 1,
            keycode: None,
            scancode: Some(Scancode::Return),
            keymod: Mod::NOMOD,
            repeat: false,
        };
        assert_eq!(ev(up), Some(Event::Key { key: Key::ENTER, pressed: false, repeat: false, mods: Mods::default() }));
        assert_eq!(
            ev(Sdl::KeyDown {
                timestamp: 0,
                window_id: 1,
                keycode: None,
                scancode: None,
                keymod: Mod::NOMOD,
                repeat: false
            }),
            None
        );
        assert_eq!(ev(Sdl::TextInput { timestamp: 0, window_id: 1, text: "é".into() }), Some(Event::Text("é".into())));
        let finger = |f: fn(u32, i64, i64, f32, f32, f32, f32, f32) -> Sdl| f(0, 1, 7, 0.5, 0.25, 0.0, 0.0, 1.0);
        let fd = finger(|t, tid, fid, x, y, dx, dy, p| Sdl::FingerDown {
            timestamp: t,
            touch_id: tid,
            finger_id: fid,
            x,
            y,
            dx,
            dy,
            pressure: p,
        });
        let fm = finger(|t, tid, fid, x, y, dx, dy, p| Sdl::FingerMotion {
            timestamp: t,
            touch_id: tid,
            finger_id: fid,
            x,
            y,
            dx,
            dy,
            pressure: p,
        });
        let fu = finger(|t, tid, fid, x, y, dx, dy, p| Sdl::FingerUp {
            timestamp: t,
            touch_id: tid,
            finger_id: fid,
            x,
            y,
            dx,
            dy,
            pressure: p,
        });
        for (e, phase) in [(fd, TouchPhase::Down), (fm, TouchPhase::Move), (fu, TouchPhase::Up)] {
            assert_eq!(ev(e), Some(Event::Touch { id: 7, phase, x: 400.0, y: 100.0 }));
        }
        assert_eq!(ev(Sdl::AppDidEnterBackground { timestamp: 0 }), None);
    }

    /// Needs a display or `SDL_VIDEODRIVER=offscreen` with Mesa: opens a window, then drives the real GLES driver
    /// through the whole stack (JIT call -> thunk -> marshalling -> driver) and reads the pixels back.
    #[test]
    #[ignore = "needs SDL2 video (xvfb-run, or SDL_VIDEODRIVER=offscreen) and a GLES2 driver"]
    fn real_window_renders_through_the_hle() {
        use bhpc_cpu::{Cpu, CpuOptions};
        use bhpc_gles::{Gles, GlesConfig};
        use bhpc_mem::GuestMemory;
        let (mut backend, gl) = SdlBackend::new(&WindowConfig { size: (64, 48), ..WindowConfig::default() }).unwrap();
        eprintln!("video driver: {}", backend.driver());
        let ctx = gl.create_context(None).expect("GLES2 context");
        assert!(gl.make_current(Some(ctx)));
        assert_eq!(backend.metrics().logical, (64, 48));
        let (w, h) = gl.drawable_size();
        assert!(w >= 64 && h >= 48);
        let shared = gl.create_context(Some(ctx)).expect("shared context");
        assert_ne!(shared, ctx);
        assert!(gl.make_current(Some(ctx)), "the previous context is restored after creating a shared one");
        gl.destroy_context(shared);

        let mem = Arc::new(GuestMemory::new().unwrap());
        mem.map(0x5000_0000, 0x10_0000).unwrap();
        mem.map(0x7000_0000, 0x10_0000).unwrap();
        let hle = Gles::with_host(GlesConfig::new(0x6000_0000, 1 << 20), gl.clone(), mem.clone()).expect("hle");
        let mut cpu = Cpu::with_options(mem.clone(), CpuOptions::default()).unwrap();
        cpu.set_sp(0x7010_0000 - 16);
        let mut call = |name: &str, args: &[u32]| {
            let a = bhpc_gles::resolve(&mut cpu, &hle, name).unwrap();
            cpu.call(a, args).unwrap().r0
        };
        let f = |v: f32| v.to_bits();
        call("glViewport", &[0, 0, 64, 48]);
        call("glClearColor", &[f(1.0), f(0.0), f(0.5), f(1.0)]);
        call("glClear", &[0x4000]);
        let buf = 0x5000_0100;
        call("glPixelStorei", &[0x0D05, 1]);
        call("glReadPixels", &[3, 4, 2, 2, 0x1908, 0x1401, buf]);
        assert_eq!(call("glGetError", &[]), 0);
        let px = mem.read_bytes(buf, 16).unwrap();
        for p in px.chunks(4) {
            assert_eq!(p[0], 255);
            assert_eq!(p[1], 0);
            assert!((126..=129).contains(&p[2]), "blue {}", p[2]);
        }
        let s = call("glGetString", &[0x1F02]);
        let version = mem.read_cstr(s).unwrap();
        eprintln!("GL_VERSION as seen by the guest: {version}");
        assert!(version.starts_with("OpenGL ES 2.0"));
        let _ = gl.set_swap_interval(0); // software drivers may refuse
        gl.swap_buffers();
        backend.present();
        assert!(backend.set_fullscreen(false));
        let _ = backend.set_vsync(false);
        backend.set_title("test");
        let _ = backend.poll_event(Duration::ZERO); // whatever the window system queued
        assert!(!gl.proc_address("glClear").is_null());
        assert!(gl.proc_address("nul\0byte").is_null());
        drop(backend);
        assert!(!gl.make_current(Some(ctx)));
        assert_eq!(gl.drawable_size(), (0, 0));
        assert!(gl.create_context(None).is_none());
        gl.swap_buffers();
        gl.destroy_context(ctx);
    }
}
