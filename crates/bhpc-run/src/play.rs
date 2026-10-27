//! `bhpc-run --apk <apk>` without `--init-only`: open an SDL window with a GLES2 context, wire GL, OpenSL ES,
//! `dlopen` and the fake JVM into the loader, run the Java-side start-up sequence and pump input.
//!
//! Threads: the host main thread owns the window and its event pump (`bhpc-app`); the "guest main" host thread
//! owns the [`Session`] and acts as the Android UI thread (start-up stages, input delivery); `MainThread.nativeRun`
//! runs on a real guest thread (a `pthread_create`d thunk with a big stack).
//!
//! Not part of the coverage gate (needs a display); the pieces with logic live in `png`, `input` and `dl`.

use crate::apkrun::{backtrace_lines, HostHooks, InitConfig, Session};
use crate::dl::{self, DlTable};
use crate::embed::{cmd_event, frame_pixels, InputTracker};
use crate::input::{host_key, JniSink};
use crate::script::{click_events, Action, Step, WAIT_TIMEOUT_MS};
use crate::tracks::Tracks;
use crate::{framestats, png, RunError};
use bhpc_app::sdl::SdlBackend;
use bhpc_app::{App, AppOptions, Backend, Event, ForwardHandler, Metrics, SystemClock, WindowConfig};
use bhpc_audio::{Mixer, Sles};
use bhpc_bionic::{lock, sched};
use bhpc_cpu::Cpu;
use bhpc_gles::{GlWindow, Gles, GlesConfig};
use bhpc_input::{Bindings, Config as InputConfig, Geometry, HostAction, Input, InputMapper};
use bhpc_jni::{GfxHooks, Stage, Throw, Vm};
use bhpc_launcher::embed::{Cmd, FrameWriter, Layout};
use bhpc_launcher::progress::{emit, report as progress, Line, Phase};
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Guest address of the GL string pool / buffer shadow arena (8 MiB).
pub const GLES_ARENA: u32 = 0x6F00_0000;
/// Stack of the guest thread that runs `MainThread.nativeRun`.
pub const MAIN_STACK: u32 = 32 << 20;
/// Cycle budget handed to a guest call (effectively unlimited).
const FOREVER: u64 = u64::MAX >> 2;

/// Everything `play` is configured with.
#[derive(Debug, Clone)]
pub struct PlayConfig {
    pub init: InitConfig,
    /// Window size in logical pixels.
    pub window: (u32, u32),
    /// Size of the window's drawable in pixels (differs from `window` on high-DPI displays); `(0, 0)` until `play`
    /// has asked the window.
    pub drawable: (u32, u32),
    /// Write the frame (PNG) here when swap number `shot_at` happens; the first frame goes to `<stem>-first.png`.
    pub shot: Option<PathBuf>,
    pub shot_at: u64,
    /// Leave once the screenshot was taken.
    pub exit_after_shot: bool,
    /// Leave after this many seconds.
    pub seconds: Option<u64>,
    /// Log `glGetError` after every GL call.
    pub check_gl: bool,
    /// Guest addresses to symbolise at the end of the run (debug aid).
    pub symbolize: Vec<u32>,
    /// Log the selector of every Objective-C method-cache miss (`lookUpMethod` hook), a cheap trace of the game's
    /// message flow: each (class, selector) pair is reported once.
    pub trace_objc: bool,
    /// Exported ARM functions whose calls are logged (`--trace-fn`).
    pub trace_fn: Vec<String>,
    /// Answer the alerts with buttons only at once instead of showing them (`--auto-alerts`; headless runs).
    pub auto_alerts: bool,
    /// Button index to press when alerts are answered automatically (default: the first button, else the cancel
    /// button).
    pub alert_choice: Option<i32>,
    /// Scripted input, screenshots and frame-time marks (see `script`).
    pub script: Vec<Step>,
    /// Directory of the screenshots a script asks for.
    pub shot_dir: PathBuf,
    /// Write the full list of Java members the guest looked up (`host`, `native`, `STUB`, call counts) here.
    pub jni_report: Option<PathBuf>,
    /// Start in fullscreen.
    pub fullscreen: bool,
    /// The game may use vsync.
    pub vsync: bool,
    /// Present at most this many frames per second (`None`: no cap besides vsync).
    pub fps_limit: Option<u32>,
    /// Draw the FPS counter over the game picture.
    pub show_fps: bool,
    /// Corner of the game picture the FPS counter sits in.
    pub fps_position: bhpc_app::hud::Corner,
    /// Let the game draw at every display refresh instead of every second one (`fpsunlock`).
    pub unlock_fps: bool,
    /// World speed multiplier, single-player only (`gamespeed`; 1: no hook).
    pub game_speed: f64,
    /// How many points the game gets for the window (`aspect::ui_scale`).
    pub zoom: crate::aspect::GameZoom,
    /// The launcher's game UI scale on top of `zoom` (`aspect::ui_scale`; 1: the zoom's points).
    pub ui_scale: f32,
    /// Game controllers drive the game (`bhpc_input::pad`).
    pub controller: bool,
    /// The game's D-pad stays hidden (`gameui`).
    pub hide_dpad: bool,
    /// The controller cursor moves tile by tile in a world (`bhpc_input::pad`).
    pub pad_tile_cursor: bool,
    /// Window title.
    pub title: String,
    /// The client mod loader (`None`: off, `--no-mods`).
    pub mods: Option<crate::modglue::ModsOptions>,
    /// Language of the messages shown over the game.
    pub lang: bhpc_app::hud::Lang,
    /// What the buttons, wheel and keys do.
    pub bindings: Bindings,
    /// Render for the launcher's window instead of one of our own: the frame file the launcher created. The video
    /// driver is forced to SDL's offscreen one, input arrives as `bhpc_launcher::embed::Cmd` lines on stdin and the
    /// run ends when stdin closes.
    pub embed: Option<PathBuf>,
    /// A link to hand the game once it runs (`--open-uri`; `VerdeActivity.nativeHandleUri`, e.g. a join link).
    pub open_uri: Option<String>,
    /// A game in a window of its own listens to the launcher on stdin (`--control`): today `mods-reload` only.
    pub control: bool,
}

impl PlayConfig {
    pub fn new(init: InitConfig) -> PlayConfig {
        PlayConfig {
            init,
            window: (1024, 768),
            drawable: (0, 0),
            shot: None,
            shot_at: 120,
            exit_after_shot: false,
            seconds: None,
            check_gl: false,
            symbolize: Vec::new(),
            jni_report: None,
            trace_objc: false,
            trace_fn: Vec::new(),
            auto_alerts: false,
            alert_choice: None,
            script: Vec::new(),
            shot_dir: PathBuf::from("."),
            fullscreen: false,
            vsync: true,
            fps_limit: None,
            show_fps: false,
            fps_position: bhpc_app::hud::Corner::TopLeft,
            unlock_fps: false,
            game_speed: 1.0,
            zoom: crate::aspect::GameZoom::Tablet,
            ui_scale: 1.0,
            controller: true,
            hide_dpad: false,
            pad_tile_cursor: false,
            title: "The Blockheads".into(),
            mods: None,
            lang: bhpc_app::hud::Lang::En,
            bindings: Bindings::default(),
            embed: None,
            open_uri: None,
            control: false,
        }
    }
}

/// What a run achieved.
#[derive(Debug, Clone, Default)]
pub struct PlayReport {
    /// `VerdeActivity.swapBuffers` + `eglSwapBuffers` calls that presented a frame.
    pub swaps: u64,
    /// Why the run ended.
    pub stop: String,
    /// Backtrace of a guest fault, innermost first.
    pub backtrace: Vec<String>,
    /// `Class.method sig` of Java members that are still stubs and were called.
    pub java_stubs: Vec<String>,
    /// GL calls that reached an ignored stub.
    pub gl_unsupported: BTreeMap<String, u64>,
    /// Imports bound to traps that the guest called.
    pub unimplemented_hits: Vec<String>,
    /// Most called host functions.
    pub top_thunks: Vec<(String, u64)>,
}

/// Registers of each guest thread at its latest call to a blocking function (to see where a hang is).
type BlockLog = Arc<Mutex<BTreeMap<u32, (String, [u32; 16])>>>;

/// Blocking libc calls recorded in the [`BlockLog`].
const BLOCKING: &[&str] = &[
    "pthread_cond_wait",
    "pthread_cond_timedwait",
    "pthread_cond_timedwait_relative_np",
    "pthread_join",
    "sem_wait",
    "usleep",
    "nanosleep",
    "sleep",
    "poll",
    "select",
    "read",
];

fn watch_blocking(cpu: &Cpu, st: &bhpc_bionic::Shared, log: &BlockLog) {
    for name in BLOCKING {
        let Some(inner) = bhpc_bionic::builtin(name, st) else { continue };
        let (log, n) = (log.clone(), (*name).to_string());
        let wrapper: bhpc_cpu::SharedThunkFn = Arc::new(move |c: &mut Cpu| {
            let regs: [u32; 16] = std::array::from_fn(|i| c.reg(i));
            log.lock().unwrap_or_else(PoisonError::into_inner).insert(sched::ctx().tid, (n.clone(), regs));
            inner(c);
        });
        let _ = cpu.register_shared_thunk(name, wrapper);
        if bhpc_bionic::gate_free(name) {
            cpu.table().set_gate_free(name);
        }
    }
}

#[derive(Default)]
struct Flags {
    done: AtomicBool,
    /// In-window mode: the launcher's view draws with GL and takes frames as read back ([`Layout::GlRgba`]).
    gl_layout: AtomicBool,
    swaps: AtomicU64,
    shot_taken: AtomicBool,
    /// Screenshots a script asked for (written at the next presented frame).
    shot_requests: Mutex<Vec<PathBuf>>,
    /// Timestamp of every presented frame and the script marks (index of the next frame).
    swap_times: Mutex<Vec<Instant>>,
    marks: Mutex<Vec<(String, usize)>>,
    /// A script asked to quit.
    script_quit: AtomicBool,
    /// A script asked for a view-tree dump.
    dump_views: AtomicBool,
    /// The fake JVM, for scripts that wait for views.
    vm: Mutex<Option<Vm>>,
    /// Where the game's surface sits in the window (kept current by the UI thread), for scripted clicks on controls.
    geometry: Mutex<Option<Geometry>>,
    /// A script asked for this window size (logical pixels); the host main thread applies it.
    resize: Mutex<Option<(u32, u32)>>,
    /// The fullscreen binding was used; the host main thread switches.
    toggle_fullscreen: AtomicBool,
    /// The UI thread finished the Java start-up stages that must precede `nativeRun`.
    startup_done: AtomicBool,
    /// Launch steps done so far (`nativeRun` entered, four lifecycle stages), for the progress lines.
    launch_steps: AtomicU32,
    /// Why the UI thread ended (set when it ends normally).
    ui_stop: Mutex<Option<String>>,
    /// Registers of the faulting guest thread.
    fail: Mutex<Option<(String, [u32; 16])>>,
    /// The controller cursor and the host's messages, drawn over the frame.
    pad_view: Mutex<crate::overlay::PadView>,
    /// Text the game copied, for the host main thread to put on the clipboard (SDL's clipboard is main-thread only).
    clipboard: Mutex<Option<String>>,
    /// The mod loader's view and the player's answers (drawn and handled on the window thread).
    mods: Mutex<Option<Arc<crate::modglue::Shared>>>,
}

/// A failure to get a window or GL context: in-window mode reports that it cannot work here (the launcher falls back
/// to a window of our own), a normal run just fails.
fn no_window(cfg: &PlayConfig, e: String) -> RunError {
    if cfg.embed.is_some() {
        RunError::EmbedUnavailable(e)
    } else {
        other(e)
    }
}

fn other<E: std::fmt::Display>(e: E) -> RunError {
    RunError::Io(std::io::Error::other(e.to_string()))
}

/// The window system as the fake `VerdeActivity` sees it.
struct Hooks {
    gles: Gles,
    window: Arc<dyn GlWindow>,
    surface: u32,
    primary: u32,
    /// Java context id -> EGL context handle.
    ctxs: Mutex<HashMap<u32, u32>>,
    next_id: AtomicU32,
    flags: Arc<Flags>,
    shot: Option<PathBuf>,
    shot_at: u64,
    overlay: crate::overlay::OverlayFeed,
    composer: Mutex<crate::overlay::Composer>,
    /// In-window mode: where finished frames go.
    embed: Option<Mutex<EmbedOut>>,
    /// The SDL window, for the read-back that does not depend on the window's own surface (in-window mode).
    sdl: Arc<bhpc_app::sdl::SdlGl>,
    /// The frame-rate cap and the FPS counter.
    pace: Mutex<crate::pace::SwapPacer>,
    /// Pictures of the controller cursor and the messages.
    sprites: Mutex<crate::overlay::SpriteCache>,
    /// The audio output, held silent until the first frame is presented.
    mixer: Mixer,
}

/// Frame file and scratch buffers of the in-window mode.
struct EmbedOut {
    /// Shared with the read-back thread.
    out: Arc<Mutex<FrameOut>>,
    last: Option<Instant>,
    /// Read-back buffer of the synchronous path, reused.
    rgba: Vec<u8>,
    /// The asynchronous read-back (`None`: off, or the driver cannot), and where its frames go.
    #[cfg(unix)]
    readback: Option<(bhpc_app::readback::Readback, bhpc_app::readback::Sink)>,
}

/// The writing end of the frame path.
struct FrameOut {
    writer: FrameWriter,
    /// Conversion buffer for the BGRA layout, reused.
    bgra: Vec<u8>,
    /// Frames published, time spent converting and writing them, and when this report period began.
    stats: (u64, Duration, Instant),
}

impl FrameOut {
    /// Publish a read-back frame (bottom-up RGBA) in the launcher's layout.
    fn publish(&mut self, w: u32, h: u32, rgba: &[u8], layout: Layout, path: &str, render_time: &Mutex<Duration>) {
        let began = Instant::now();
        let FrameOut { writer, bgra, stats } = self;
        match writer.publish_as(w, h, layout, frame_pixels(layout, w, h, rgba, bgra)) {
            Ok(true) => {
                stats.0 += 1;
                stats.1 += began.elapsed();
                if stats.2.elapsed() >= Duration::from_secs(10) {
                    let n = stats.0.max(1) as f64;
                    let render = std::mem::take(&mut *render_time.lock().unwrap_or_else(PoisonError::into_inner));
                    eprintln!(
                        "[embed] frame path: {path}, layout {layout:?}, {w}x{h}, {:.1} fps, {:.2} ms per frame on the render thread, {:.2} ms converting and writing",
                        stats.0 as f64 / stats.2.elapsed().as_secs_f64(),
                        render.as_secs_f64() * 1000.0 / n,
                        stats.1.as_secs_f64() * 1000.0 / n,
                    );
                    *stats = (0, Duration::ZERO, Instant::now());
                }
            }
            Ok(false) => eprintln!("[play] a {w}x{h} frame does not fit the frame file"),
            Err(e) => eprintln!("[play] cannot write the frame file: {e}"),
        }
    }
}

/// Render-thread time per report period, shared by the paths (the report is printed where frames are written).
static RENDER_TIME: Mutex<Duration> = Mutex::new(Duration::ZERO);

/// The smallest edge the first reported size may have.
const MIN_FIRST_EDGE: u32 = 64;

/// How long the runner waits for the launcher to say how big its view is.
const FIRST_SIZE_WAIT: Duration = Duration::from_secs(5);

/// At most this many frames per second go to the launcher.
const EMBED_FRAME_GAP: Duration = Duration::from_millis(15);

type ReadPixels = unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void);
type GetIntegerv = unsafe extern "C" fn(u32, *mut i32);

impl Hooks {
    fn egl(&self, java_id: u32) -> Option<u32> {
        self.ctxs.lock().unwrap_or_else(PoisonError::into_inner).get(&java_id).copied()
    }

    /// Read the default framebuffer back (RGBA, top row first). The context must be current on this thread.
    fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.capture_raw().map(|(w, h, buf)| (w, h, png::flip_rows(w, h, &buf)))
    }

    /// Like `capture`, rows as OpenGL returns them (bottom row first).
    fn capture_raw(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (w, h) = self.window.drawable_size();
        let (rp, gi) = (self.window.proc_address("glReadPixels"), self.window.proc_address("glGetIntegerv"));
        if rp.is_null() || gi.is_null() || w == 0 || h == 0 {
            return None;
        }
        // SAFETY: both pointers come from the driver for these exact signatures; the buffer is w*h*4 bytes.
        unsafe {
            let (rp, gi): (ReadPixels, GetIntegerv) = (
                std::mem::transmute::<*const c_void, ReadPixels>(rp),
                std::mem::transmute::<*const c_void, GetIntegerv>(gi),
            );
            // The window as the user sees it: present the game's surface (letterboxed) into the back buffer first.
            self.window.compose();
            let bind = self.window.proc_address("glBindFramebuffer");
            if bind.is_null() {
                return None;
            }
            let bind = std::mem::transmute::<*const c_void, unsafe extern "C" fn(u32, u32)>(bind);
            let mut fbo = 0;
            gi(0x8CA6, &mut fbo);
            bind(0x8D40, 0);
            let mut buf = vec![0u8; w as usize * h as usize * 4];
            rp(0, 0, w as i32, h as i32, 0x1908, 0x1401, buf.as_mut_ptr().cast());
            bind(0x8D40, fbo as u32);
            Some((w, h, buf))
        }
    }

    /// Hand the frame to the launcher (in-window mode), at most about 60 times a second.
    fn publish_frame(&self) {
        let Some(embed) = &self.embed else { return };
        let mut e = embed.lock().unwrap_or_else(PoisonError::into_inner);
        if e.last.is_some_and(|t| t.elapsed() < EMBED_FRAME_GAP) {
            return;
        }
        let began = Instant::now();
        let layout = if self.flags.gl_layout.load(Ordering::SeqCst) { Layout::GlRgba } else { Layout::Bgra };
        let e = &mut *e;
        #[cfg(unix)]
        if let Some((rb, sink)) = e.readback.as_mut() {
            use bhpc_app::readback::Started;
            match self.sdl.compose_read_async(rb, sink) {
                Some(Started::Queued) => {
                    e.last = Some(Instant::now());
                    *RENDER_TIME.lock().unwrap_or_else(PoisonError::into_inner) += began.elapsed();
                    return;
                }
                Some(Started::Busy) | None => return,
                Some(Started::Unsupported) => e.readback = None,
            }
        }
        let Some((w, h)) = self.sdl.compose_read(&mut e.rgba) else { return };
        e.last = Some(Instant::now());
        *RENDER_TIME.lock().unwrap_or_else(PoisonError::into_inner) += began.elapsed();
        let mut out = e.out.lock().unwrap_or_else(PoisonError::into_inner);
        out.publish(w, h, &e.rgba, layout, "synchronous read-back", &RENDER_TIME);
    }

    /// Count a presented frame (and take the screenshot when it is due); call right before the swap.
    fn before_swap(&self) {
        // The frame is drawn: wait for its slot under the frame-rate cap (other guest threads keep running), then
        // present it. The game measures real time between frames, so a capped game just sees longer frames.
        let height = self.window.surface_size().1.min(self.window.drawable_size().1);
        if let Some(hud) = self.pace.lock().unwrap_or_else(PoisonError::into_inner).frame(height) {
            self.sdl.set_hud(Some(hud));
        }
        {
            let view = self.flags.pad_view.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let (sw, sh) = self.window.surface_size();
            let (dw, dh) = self.window.drawable_size();
            // The game picture as the window letterboxes it.
            let fit = (dw as f32 / sw.max(1) as f32).min(dh as f32 / sh.max(1) as f32);
            let picture = ((sw as f32 * fit) as u32, (sh as f32 * fit) as u32);
            let sprites =
                self.sprites.lock().unwrap_or_else(PoisonError::into_inner).sprites(&view, picture, Instant::now());
            self.sdl.set_sprites(sprites);
        }
        // The Java views (popups) go over the game's frame, also in screenshots.
        let vm = self.flags.vm.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(vm) = vm {
            // The mod UI (join prompt, F9 panel, toasts) goes over the views.
            let mods = self.flags.mods.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let top = mods.and_then(|m| {
                let (w, h) = vm.screen_size();
                let (w, h) = (w.max(1) as u32, h.max(1) as u32);
                let view = m.view.lock().unwrap_or_else(PoisonError::into_inner);
                let serial = m.serial.load(Ordering::SeqCst);
                let px = m.ui.lock().unwrap_or_else(PoisonError::into_inner).render(&view, serial, w, h);
                px.map(|p| (p, w, h))
            });
            let frame = self.composer.lock().unwrap_or_else(PoisonError::into_inner).compose(vm.render_ui(), top);
            self.overlay.update(&*self.window, frame);
        }
        self.publish_frame();
        let n = self.flags.swaps.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            emit(Line::Ready);
            // The loading screen goes away now: the sound the game started while loading (its menu music) plays
            // from here, from its beginning.
            self.mixer.set_held(false);
        }
        self.flags.swap_times.lock().unwrap_or_else(PoisonError::into_inner).push(Instant::now());
        let wanted: Vec<PathBuf> =
            std::mem::take(&mut *self.flags.shot_requests.lock().unwrap_or_else(PoisonError::into_inner));
        if !wanted.is_empty() {
            self.write_shots(n, &wanted);
        }
        self.snapshot(n);
    }

    fn write_shots(&self, n: u64, paths: &[PathBuf]) {
        match self.capture() {
            Some((w, h, px)) => {
                for p in paths {
                    let lit = px.chunks(4).filter(|p| p[0] | p[1] | p[2] != 0).count();
                    eprintln!("[play] frame {n}: {w}x{h}, {lit} non-black pixels -> {}", p.display());
                    if let Err(e) = png::write_file(p, w, h, &px) {
                        eprintln!("[play] cannot write {}: {e}", p.display());
                    }
                }
            }
            None => eprintln!("[play] frame {n}: read-back not possible"),
        }
    }

    fn snapshot(&self, n: u64) {
        let Some(path) = &self.shot else { return };
        let target = if n == self.shot_at {
            path.clone()
        } else if n == 1 {
            path.with_file_name(format!(
                "{}-first.png",
                path.file_stem().map_or("frame".into(), |s| s.to_string_lossy())
            ))
        } else {
            return;
        };
        match self.capture() {
            Some((w, h, px)) => {
                let lit = px.chunks(4).filter(|p| p[0] | p[1] | p[2] != 0).count();
                eprintln!("[play] frame {n}: {w}x{h}, {lit} non-black pixels -> {}", target.display());
                if let Err(e) = png::write_file(&target, w, h, &px) {
                    eprintln!("[play] cannot write {}: {e}", target.display());
                }
                if n == self.shot_at {
                    self.flags.shot_taken.store(true, Ordering::SeqCst);
                }
            }
            None => eprintln!("[play] frame {n}: read-back not possible (offscreen framebuffer bound?)"),
        }
    }
}

impl GfxHooks for Hooks {
    fn swap_buffers(&self) -> bool {
        self.before_swap();
        self.gles.egl_swap_buffers(self.surface).is_ok()
    }

    fn make_current(&self, ctx: u32) -> bool {
        if ctx == 0 {
            return self.gles.egl_make_current(0, 0, 0).is_ok();
        }
        match self.egl(ctx) {
            Some(c) => self.gles.egl_make_current(self.surface, self.surface, c).is_ok(),
            None => false,
        }
    }

    fn create_shared_context(&self, _version: i32) -> u32 {
        match self.gles.egl_create_context(self.primary) {
            Ok(c) => {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                self.ctxs.lock().unwrap_or_else(PoisonError::into_inner).insert(id, c);
                id
            }
            Err(_) => 0,
        }
    }

    fn destroy_context(&self, ctx: u32) -> bool {
        let c = self.ctxs.lock().unwrap_or_else(PoisonError::into_inner).remove(&ctx);
        c.is_some_and(|c| self.gles.egl_destroy_context(c).is_ok())
    }

    fn set_content_size(&self, w: f32, h: f32) {
        eprintln!("[play] content size {w}x{h}");
        self.window.set_surface_size(w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    }

    fn context_lost(&self) {
        eprintln!("[play] game reported a lost EGL context");
    }
}

/// Backend wrapper that turns the shared "done" flag into a `Quit` event, so the event loop ends when the guest does.
struct Watched {
    inner: SdlBackend,
    flags: Arc<Flags>,
    fullscreen: bool,
}

impl Backend for Watched {
    fn poll_event(&mut self, timeout: Duration) -> Option<Event> {
        if self.flags.done.load(Ordering::SeqCst) {
            return Some(Event::Quit);
        }
        // A scripted `resize`: the window system answers with a `Resized` event like it does for a drag.
        if let Some((w, h)) = self.flags.resize.lock().unwrap_or_else(PoisonError::into_inner).take() {
            eprintln!("[play] resizing the window to {w}x{h}: {}", self.inner.set_size(w, h));
        }
        if let Some(text) = self.flags.clipboard.lock().unwrap_or_else(PoisonError::into_inner).take() {
            if !self.inner.set_clipboard_text(&text) {
                eprintln!("[play] cannot set the clipboard");
            }
        }
        if self.flags.toggle_fullscreen.swap(false, Ordering::SeqCst) {
            let want = !self.fullscreen;
            if self.inner.set_fullscreen(want) {
                self.fullscreen = want;
            }
            eprintln!("[play] fullscreen {}", if self.fullscreen { "on" } else { "off" });
        }
        self.inner.poll_event(timeout)
    }
    fn metrics(&self) -> Metrics {
        self.inner.metrics()
    }
    fn set_fullscreen(&mut self, on: bool) -> bool {
        let ok = self.inner.set_fullscreen(on);
        if ok {
            self.fullscreen = on;
        }
        ok
    }
    fn present(&mut self) {
        self.inner.present();
    }
    fn set_vsync(&mut self, on: bool) -> bool {
        self.inner.set_vsync(on)
    }
    fn set_title(&mut self, title: &str) {
        self.inner.set_title(title);
    }
}

/// Run the game. Blocks until the window closes, the game ends or a limit is reached.
pub fn play(cfg: &PlayConfig) -> Result<PlayReport, RunError> {
    let flags = Arc::new(Flags::default());
    let (tx, rx) = channel();
    let mut cfg = cfg.clone();
    if cfg.embed.is_some() {
        // No window of our own: SDL's offscreen driver gives the GL context, the launcher shows the frames. It needs
        // EGL, which Windows and macOS do not have: there a hidden window gives the context (WGL / CGL) instead.
        if cfg!(target_os = "linux") {
            std::env::set_var("SDL_VIDEODRIVER", "offscreen");
        }
        // macOS: no Dock icon or menu bar for the hidden window's process.
        std::env::set_var("SDL_MAC_BACKGROUND_APP", "1");
        // An offscreen window never has the focus, and SDL drops controller input of unfocused applications; the
        // launcher's focus commands decide instead (`bhpc_input::pad`).
        std::env::set_var("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        // The game picks its content size from the screen size once, so start with the size of the launcher's view
        // (its first `size` command) instead of a guess.
        let (first_tx, first_rx) = channel();
        spawn_commands(tx.clone(), flags.clone(), first_tx);
        match first_rx.recv_timeout(FIRST_SIZE_WAIT) {
            Ok(size) => cfg.window = size,
            Err(_) => eprintln!("[play] no size from the launcher yet; starting with {:?}", cfg.window),
        }
    }
    let cfg = &cfg;
    let (mut backend, gl) = SdlBackend::new(&WindowConfig {
        title: cfg.title.clone(),
        size: cfg.window,
        controllers: cfg.controller,
        hidden: cfg.embed.is_some(),
        ..WindowConfig::default()
    })
    .map_err(|e| no_window(cfg, e))?;
    gl.set_vsync_allowed(cfg.vsync);
    let mut icon = include_bytes!("../assets/icon-32.rgba").to_vec();
    backend.set_icon(&mut icon, 32, 32);
    let m0 = backend.metrics();
    eprintln!("[play] video driver {}, window {:?}, drawable {:?}", backend.driver(), m0.logical, m0.drawable);
    // What the window system really gave us is what the game sees (a compositor may ignore the requested size).
    let mut cfg = cfg.clone();
    (cfg.window, cfg.drawable) = (m0.logical, m0.drawable);
    // The game takes its UI scale from the first screen size: starting fullscreen, that is the desktop's size (the
    // window only becomes fullscreen once the event loop runs).
    if cfg.fullscreen && cfg.embed.is_none() {
        if let Some(d) = backend.desktop_size() {
            (cfg.window, cfg.drawable) = (d, crate::aspect::drawable_for(d, m0.logical, m0.drawable));
            eprintln!("[play] starting fullscreen: the game starts with the desktop size {:?}", cfg.drawable);
        }
    }
    let cfg = &cfg;
    let mixer = Mixer::new(44100);
    // Silent until the game presents its first frame (`Hooks::before_swap`): it starts its music while loading.
    mixer.set_held(true);
    let m = mixer.clone();
    if let Err(e) = backend.open_audio(44100, move |out| m.fill(out)) {
        // No audio device: drain the mixer in real time so the game's writers still make progress.
        eprintln!("[play] no audio device ({e}); audio is discarded");
        let (m, f) = (mixer.clone(), flags.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0i16; 2 * 441];
            while !f.done.load(Ordering::SeqCst) {
                m.fill(&mut buf);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    }
    if cfg.control && cfg.embed.is_none() {
        spawn_control(flags.clone());
    }
    if !cfg.script.is_empty() {
        let (script, tx, f, dir) = (cfg.script.clone(), tx.clone(), flags.clone(), cfg.shot_dir.clone());
        std::thread::spawn(move || run_script(&script, &tx, &f, &dir));
    }
    let (c, f, mx) = (cfg.clone(), flags.clone(), mixer);
    let guest = std::thread::Builder::new()
        .name("guest-main".into())
        .stack_size(256 << 20)
        .spawn(move || guest_main(&c, gl, rx, f, mx))?;
    let mut app = App::new(
        Watched { inner: backend, flags: flags.clone(), fullscreen: false },
        SystemClock::new(),
        // The fullscreen key is a binding now (`HostAction::ToggleFullscreen` from the input mapper). The game's
        // swap interval is set through the GL window (`set_vsync_allowed`); the loop itself does not render.
        AppOptions {
            render_in_loop: false,
            vsync: false,
            fallback_fps: None,
            fullscreen: cfg.fullscreen && cfg.embed.is_none(),
            fullscreen_keys: false,
            ..AppOptions::default()
        },
    );
    app.run(&mut ForwardHandler::new(tx));
    flags.done.store(true, Ordering::SeqCst);
    let report = guest.join().map_err(|_| other("the guest main thread panicked"))?;
    drop(app);
    report
}

/// In-window mode: read the launcher's commands from stdin. A size becomes a window resize, everything else a window
/// event; end of input (the launcher is gone) or `quit` ends the run.
fn spawn_commands(
    tx: std::sync::mpsc::Sender<Event>,
    flags: Arc<Flags>,
    first_size: std::sync::mpsc::Sender<(u32, u32)>,
) {
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::stdin().lock()) {
            let Ok(line) = line else { break };
            match Cmd::parse(&line) {
                Some(Cmd::Size { width, height }) => {
                    // A view that is not laid out yet reports a sliver; the game must not start with that.
                    if width >= MIN_FIRST_EDGE && height >= MIN_FIRST_EDGE {
                        let _ = first_size.send((width, height));
                    }
                    *flags.resize.lock().unwrap_or_else(PoisonError::into_inner) = Some((width.max(1), height.max(1)));
                }
                Some(Cmd::Quit) => break,
                Some(Cmd::Layout(l)) => flags.gl_layout.store(l == Layout::GlRgba, Ordering::SeqCst),
                Some(Cmd::ModsReload) => {
                    if let Some(m) = flags.mods.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                        m.post(bhpc_mods::view::UiCmd::Reload);
                    }
                }
                Some(Cmd::ModsChat(line)) => {
                    if let Some(m) = flags.mods.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                        m.post(bhpc_mods::view::UiCmd::Say(line));
                    }
                }
                Some(cmd) => {
                    if cmd_event(&cmd).is_some_and(|ev| tx.send(ev).is_err()) {
                        break;
                    }
                }
                None => eprintln!("[play] unknown command from the launcher: {line:?}"),
            }
        }
        flags.done.store(true, Ordering::SeqCst);
    });
}

/// A game in a window of its own: the launcher's control lines on stdin (it is its pipe to the game). Only
/// `mods-reload` and `mods-chat` mean something here; input and size belong to the window. The end of input just ends the reading:
/// the window keeps the game going.
fn spawn_control(flags: Arc<Flags>) {
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::stdin().lock()) {
            let Ok(line) = line else { break };
            let cmd = match Cmd::parse(&line) {
                Some(Cmd::ModsReload) => bhpc_mods::view::UiCmd::Reload,
                Some(Cmd::ModsChat(text)) => bhpc_mods::view::UiCmd::Say(text),
                _ => continue,
            };
            if let Some(m) = flags.mods.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                m.post(cmd);
            }
        }
    });
}

/// Play a script in real time: events go to the window's event channel, the rest to the shared flags.
fn run_script(script: &[Step], tx: &std::sync::mpsc::Sender<Event>, flags: &Flags, shot_dir: &std::path::Path) {
    // The clock starts at the first presented frame: start-up time varies a lot between runs.
    while flags.swaps.load(Ordering::SeqCst) == 0 {
        if flags.done.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let begun = Instant::now();
    // Time spent in `waittext`/`clicktext` pushes everything after it back.
    let mut shift = 0u64;
    for (at, action) in script {
        let due = Duration::from_millis(*at + shift);
        std::thread::sleep(due.saturating_sub(begun.elapsed()));
        match action {
            Action::Event(e) => {
                let _ = tx.send(e.clone());
            }
            Action::Shot(name) => {
                flags
                    .shot_requests
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(shot_dir.join(format!("{name}.png")));
            }
            Action::Mark(label) => {
                let at = flags.swap_times.lock().unwrap_or_else(PoisonError::into_inner).len();
                eprintln!("[script] {} ms: mark {label} (frame {at})", begun.elapsed().as_millis());
                flags.marks.lock().unwrap_or_else(PoisonError::into_inner).push((label.clone(), at));
            }
            Action::WaitText(text) | Action::ClickText(text) => {
                let found = wait_text(flags, text);
                eprintln!(
                    "[script] {} ms: {} {text:?}",
                    begun.elapsed().as_millis(),
                    if found { "found" } else { "TIMEOUT waiting for" }
                );
                if let (true, Action::ClickText(_)) = (found, action) {
                    click_text(flags, tx, text);
                }
                shift = (begun.elapsed().as_millis() as u64).saturating_sub(*at);
            }
            Action::Resize(w, h) => *flags.resize.lock().unwrap_or_else(PoisonError::into_inner) = Some((*w, *h)),
            Action::DumpViews => flags.dump_views.store(true, Ordering::SeqCst),
            Action::Quit => flags.script_quit.store(true, Ordering::SeqCst),
        }
    }
}

fn vm_of(flags: &Flags) -> Option<Vm> {
    flags.vm.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Poll until a view shows `text` (the game builds its alerts and popups on its own schedule).
fn wait_text(flags: &Flags, text: &str) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(WAIT_TIMEOUT_MS) && !flags.done.load(Ordering::SeqCst) {
        if vm_of(flags).is_some_and(|vm| vm.has_text(text)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn click_text(flags: &Flags, tx: &std::sync::mpsc::Sender<Event>, text: &str) {
    let vm = vm_of(flags);
    let geometry = flags
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .unwrap_or(Geometry { content: None, window: (0, 0) });
    let Some((x, y)) = vm
        .as_ref()
        .and_then(|vm| vm.control_screen_pos(text))
        .zip(vm.as_ref())
        .map(|((x, y), vm)| crate::input::screen_to_window(&geometry, vm.content_scale(), x, y))
    else {
        eprintln!("[script] no control shows {text:?}");
        return;
    };
    eprintln!("[script] clicking {text:?} at {x:.0},{y:.0}");
    for (delay, e) in click_events(x, y) {
        std::thread::sleep(Duration::from_millis(if delay == 0 { 0 } else { 60 }));
        let _ = tx.send(e);
    }
}

/// Pieces created while the session is being opened.
struct Parts {
    gles: Gles,
    sles: Arc<Sles>,
}

/// Links, mail, clipboard and notices for the fake JVM. Copied text goes to SDL's clipboard in a window of our own and
/// to the launcher (an `@copy` line) in the launcher's window, where SDL's offscreen driver has no clipboard.
fn desktop_hooks(flags: &Arc<Flags>, embedded: bool, pictures: Option<PathBuf>) -> Arc<dyn bhpc_jni::SystemHooks> {
    let (f1, f2) = (flags.clone(), flags.clone());
    let copy: crate::desktop::Sink = if embedded {
        Box::new(|t| println!("{}", bhpc_launcher::embed::copy_line(t)))
    } else {
        Box::new(move |t| *f1.clipboard.lock().unwrap_or_else(PoisonError::into_inner) = Some(t.to_string()))
    };
    let notice: crate::desktop::Sink = Box::new(move |t| {
        f2.pad_view.lock().unwrap_or_else(PoisonError::into_inner).message = Some((t.to_string(), Instant::now()));
    });
    let pages = std::env::temp_dir().join("blockheads-pc-pages");
    let opener = crate::desktop::Opener::from_env(&|k| std::env::var(k).ok());
    let desk = crate::desktop::Desktop::new(opener, pages, pictures, copy, notice);
    if embedded {
        // In the launcher's window pages show in place of the game, as on the phone.
        return Arc::new(desk.with_page_sink(Box::new(|h| println!("{}", bhpc_launcher::embed::page_line(h)))));
    }
    Arc::new(desk)
}

fn guest_main(
    cfg: &PlayConfig,
    gl: Arc<bhpc_app::sdl::SdlGl>,
    rx: Receiver<Event>,
    flags: Arc<Flags>,
    mixer: Mixer,
) -> Result<PlayReport, RunError> {
    let result = boot(cfg, gl, rx, &flags, mixer);
    flags.done.store(true, Ordering::SeqCst);
    result
}

fn boot(
    cfg: &PlayConfig,
    gl: Arc<bhpc_app::sdl::SdlGl>,
    rx: Receiver<Event>,
    flags: &Arc<Flags>,
    mixer: Mixer,
) -> Result<PlayReport, RunError> {
    let sdl = gl.clone();
    let window: Arc<dyn GlWindow> = gl;
    let dl_slot: Arc<Mutex<Option<DlTable>>> = Arc::new(Mutex::new(None));
    let mut parts: Option<Parts> = None;
    let blocks: BlockLog = Arc::default();
    let mut icfg = cfg.init.clone();
    icfg.watchdog_cycles = FOREVER;
    // The game's display is the drawable (physical pixels); it picks its own content size from that.
    let screen = if cfg.drawable.0 > 0 && cfg.drawable.1 > 0 { cfg.drawable } else { cfg.window };
    icfg.screen = Some((screen.0 as i32, screen.1 as i32));
    let hooks_flags = flags.clone();
    let (embedded, pictures) = (cfg.embed.is_some(), cfg.init.pictures.clone());
    let embed_cfg = cfg.clone();
    let embed_out: Mutex<Option<EmbedOut>> = Mutex::new(match &cfg.embed {
        Some(path) => {
            let writer = FrameWriter::open(path).map_err(|e| other(format!("frame file {}: {e}", path.display())))?;
            let out =
                Arc::new(Mutex::new(FrameOut { writer, bgra: Vec::new(), stats: (0, Duration::ZERO, Instant::now()) }));
            Some(EmbedOut {
                #[cfg(unix)]
                readback: crate::embed::async_readback(std::env::var("BHPC_EMBED_ASYNC").ok().as_deref()).then(|| {
                    let (out, flags) = (out.clone(), flags.clone());
                    let sink: bhpc_app::readback::Sink = Arc::new(move |w, h, px: &[u8]| {
                        let layout = if flags.gl_layout.load(Ordering::SeqCst) { Layout::GlRgba } else { Layout::Bgra };
                        let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
                        out.publish(w, h, px, layout, "asynchronous read-back (pixel buffer + fence)", &RENDER_TIME);
                    });
                    (bhpc_app::readback::Readback::new(), sink)
                }),
                out,
                last: None,
                rgba: Vec::new(),
            })
        }
        None => None,
    });
    let (shot, shot_at, check_gl) = (cfg.shot.clone(), cfg.shot_at, cfg.check_gl);
    let (fps_limit, fps_hud, lang) = (cfg.fps_limit, cfg.show_fps.then_some(cfg.fps_position), cfg.lang);
    let mut sess = Session::open_setup(&icfg, Box::new(std::io::stdout()), None, &mut |cpu, st| {
        cpu.table().set_stats(std::env::var_os("BHPC_THUNK_STATS").is_some());
        let primary = window
            .create_context(None)
            .ok_or_else(|| no_window(&embed_cfg, "cannot create a GLES2 context".to_string()))?;
        if !window.make_current(Some(primary)) {
            return Err(other("cannot make the GL context current"));
        }
        let mut gcfg = GlesConfig::new(GLES_ARENA, 8 << 20);
        gcfg.check_errors = check_gl;
        let gles = Gles::with_host(gcfg, window.clone(), cpu.mem().clone()).map_err(RunError::Mem)?;
        gles.egl_initialize();
        let pid = gles.egl_adopt_context(primary);
        let surface = gles.egl_create_surface();
        window.make_current(None);
        let mut ctxs = HashMap::new();
        ctxs.insert(1, pid);
        let hooks = Arc::new(Hooks {
            gles: gles.clone(),
            window: window.clone(),
            surface,
            primary: pid,
            ctxs: Mutex::new(ctxs),
            next_id: AtomicU32::new(2),
            flags: hooks_flags.clone(),
            shot: shot.clone(),
            shot_at,
            overlay: crate::overlay::OverlayFeed::default(),
            composer: Mutex::default(),
            sdl: sdl.clone(),
            pace: Mutex::new(crate::pace::SwapPacer::new(lock(st).sched.clone(), fps_limit, fps_hud)),
            sprites: Mutex::new(crate::overlay::SpriteCache::new(lang)),
            embed: embed_out.lock().unwrap_or_else(PoisonError::into_inner).take().map(Mutex::new),
            mixer: mixer.clone(),
        });
        // `eglSwapBuffers` called by the game itself (through `dlsym`) is also a presented frame.
        if let Some(inner) = bhpc_gles::builtin("eglSwapBuffers", &gles) {
            let h = hooks.clone();
            let _ = cpu.register_shared_thunk(
                "eglSwapBuffers",
                Arc::new(move |c: &mut Cpu| {
                    h.before_swap();
                    inner(c);
                }),
            );
        }
        bhpc_gles::register_all(cpu, &gles);
        let sles = Sles::install(cpu, mixer.clone()).map_err(other)?;
        // Before the descriptor table: it rebinds `read`/`poll`/`select` itself, and the logging wrappers must not
        // replace those with plain bionic builtins.
        watch_blocking(cpu, st, &blocks);
        let vfd = crate::vfd::install(cpu, st);
        crate::mmapio::SharedMaps::default().install(cpu, st, &vfd);
        dl::install(cpu, st, dl_slot.clone());
        parts = Some(Parts { gles, sles });
        let gfx: Arc<dyn GfxHooks> = hooks;
        let audio: Arc<dyn bhpc_jni::AudioHooks> = Arc::new(Tracks::new(mixer.clone(), st.clone()));
        Ok(HostHooks {
            gfx: Some(gfx),
            audio: Some(audio),
            system: Some(desktop_hooks(&hooks_flags, embedded, pictures.clone())),
        })
    })?;
    let Parts { gles, sles } = parts.ok_or_else(|| other("setup did not run"))?;
    let st = sess.st.clone();
    {
        let (g, s, b) = (gles.clone(), sles.clone(), st.clone());
        let host: dl::HostResolver = Box::new(move |cpu, name| {
            s.lookup(name)
                .map(|(a, _)| a)
                .or_else(|| bhpc_gles::resolve(cpu, &g, name))
                .or_else(|| dl::bionic_symbol(cpu, &b, name))
        });
        let mut table = DlTable::new(&sess.linker, &sess.host_libs, host);
        table.set_trace(cfg.init.trace_fs);
        *dl_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(table);
    }
    if cfg.trace_objc {
        if let Err(e) = hook_objc(&mut sess) {
            eprintln!("[play] objc trace unavailable: {e}");
        }
    }
    let screen_modes = follow_window_aspect(&mut sess, cfg.zoom, cfg.ui_scale)
        .inspect(|_| eprintln!("[play] the game's screen follows the window size"))
        .inspect_err(|e| eprintln!("[play] the game keeps its emulated iOS screen size: {e}"))
        .ok();
    if cfg.unlock_fps {
        match unlock_fps(&mut sess) {
            Ok(n) => eprintln!("[play] 30 FPS cap unlocked ({n} refresh-rate constants patched)"),
            Err(e) => eprintln!("[play] cannot unlock the 30 FPS cap: {e}"),
        }
    }
    if cfg.game_speed != 1.0 {
        match game_speed(&mut sess, cfg.game_speed) {
            Ok(()) => eprintln!("[play] game speed {}x (single-player)", cfg.game_speed),
            Err(e) => eprintln!("[play] cannot change the game speed: {e}"),
        }
    }
    let game = match install_game_ui(&mut sess, cfg.hide_dpad) {
        Ok(g) => {
            if cfg.hide_dpad {
                eprintln!("[play] the game's D-pad stays hidden");
            }
            Some(g)
        }
        Err(e) => {
            eprintln!("[play] the game's buttons cannot be hidden and the controller cannot walk or zoom: {e}");
            None
        }
    };
    for f in &cfg.trace_fn {
        if let Err(e) = hook_trace(&mut sess, f) {
            eprintln!("[play] cannot trace {f}: {e}");
        }
    }
    let mut report = PlayReport::default();
    let vm = sess.jni.vm.clone();
    // Off switch of the popup renderer's change tracking (redraw only what changed): BHPC_UI_DIRTY=0.
    if std::env::var_os("BHPC_UI_DIRTY").is_some_and(|v| v == "0") {
        eprintln!("[play] Java view change tracking off (BHPC_UI_DIRTY=0)");
        vm.set_ui_dirty_tracking(false);
    }
    *flags.vm.lock().unwrap_or_else(PoisonError::into_inner) = Some(vm.clone());
    let pid = sess.cpu.processor_id();

    // 1. Constructors, JNI_OnLoad and libv_init of every library.
    let (_, failures) = sess.run_constructors(&icfg)?;
    if let Some(f) = failures.first() {
        report.stop = format!("constructor failed: {} ({})", f.failure, f.stop);
        report.backtrace = f.backtrace.clone();
        return Ok(finish(sess, &gles, report, flags, cfg, &blocks));
    }
    eprintln!("[play] constructors done");
    progress(Phase::Runtime, 0, RUNTIME_STEPS);
    // Dev aid: BHPC_DEV_OBJC_DUMP=FILE (one class name per line) prints every method of those classes with its type
    // encoding, read from the class metadata (`-[Class sel] types`).
    if let Some(list) = std::env::var_os("BHPC_DEV_OBJC_DUMP") {
        dev_objc_dump(&sess, std::path::Path::new(&list));
    }
    // Dev-only proof of the host ObjC hooks (`objchook`): BHPC_DEV_OBJC_HOOKS=1.
    let dev_hooks =
        std::env::var_os("BHPC_DEV_OBJC_HOOKS").is_some_and(|v| v == "1").then(|| dev_objc_hooks(&mut sess)).flatten();
    // The client mod loader (`modglue`): hooks installed now, the rest retried from the frame hook.
    let mod_glue = cfg.mods.clone().and_then(|opts| {
        let rt = crate::objchook::Runtime::resolve(|n| sess.linker.modules().iter().find_map(|m| m.export(n)))
            .inspect_err(|e| eprintln!("[mods] no ObjC runtime, mods are off: {e}"))
            .ok()?;
        let ivars = crate::cosmetics::Ivars::resolve(|n| sess.linker.modules().iter().find_map(|m| m.export(n)))
            .inspect_err(|e| eprintln!("[cosmetics] off: {e}"))
            .ok();
        let g = Arc::new(crate::modglue::ModGlue::new(rt, opts, ivars));
        g.install(&mut sess.cpu);
        // Native mods: the libbhmod functions as thunks, the game's modules as the hooks' view of the guest.
        let natives = crate::native::Natives::new(rt, sess.st.clone(), sess.linker.modules());
        natives.register(&sess.cpu);
        crate::native::set(natives);
        Some(g)
    });
    if let Some(g) = &mod_glue {
        *flags.mods.lock().unwrap_or_else(PoisonError::into_inner) = Some(g.shared.clone());
    }

    if let Some(secs) = cfg.seconds {
        // If a guest thread is stuck in a call that never returns (so the UI loop cannot look at the clock), stop
        // the scheduler: every blocked thread wakes, the run ends and the report shows where each one waited.
        // The same limit also covers a guest that spins: the time slice checks the wall-clock deadline.
        bhpc_bionic::sched::set_wall_deadline(
            &sess.st,
            Some(std::time::Instant::now() + Duration::from_secs(secs + 5)),
        );
        let (sched, done) = (lock(&sess.st).sched.clone(), flags.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs + 5));
            if !done.done.load(Ordering::SeqCst) {
                eprintln!("[play] watchdog: stopping the scheduler");
                sched.shutdown();
            }
        });
    }

    // 2. This thread is the game's main thread: libv recorded it as `pthread_main_thread_np` during `libv_init`, and
    // `CFRunLoopGetMain` (where the Lifecycle callbacks queue their blocks) follows that. The Android UI thread
    // (Java start-up stages, looper, layout, input) therefore becomes a guest thread of its own.
    let stack = sess.cpu.mem().map_anywhere(256 << 10, 4096)?;
    let _pump = sles.spawn_pump(&sess.cpu, stack + (256 << 10))?;
    // Touch/key/text natives run on this thread, right before a frame is presented.
    let main_q = {
        let st = st.clone();
        crate::mainq::MainQueue::new(move |c| sched::sleep(c, &st, 1_000_000), 2000)
    };
    {
        let q = main_q.clone();
        let mut retry = dev_hooks.map(|(h, rt)| (h, rt, Mutex::new(Instant::now())));
        if retry.as_ref().is_some_and(|(h, _, _)| h.waiting() == 0) {
            retry = None;
        }
        let glue = mod_glue.clone().map(|g| (g, Mutex::new(Instant::now())));
        gles.set_frame_hook(Arc::new(move |c| {
            q.drain(c);
            if let Some((g, last)) = &glue {
                let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
                if g.hooks.waiting() > 0 && last.elapsed() >= Duration::from_secs(1) {
                    *last = Instant::now();
                    drop(last);
                    g.install(c);
                }
                g.frame(c);
            }
            // Waiting dev hooks: try again once a second, on the main thread.
            if let Some((h, rt, last)) = &retry {
                let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
                if h.waiting() > 0 && last.elapsed() >= Duration::from_secs(1) {
                    *last = Instant::now();
                    drop(last);
                    report_objc_hooks(&h.install(c, rt));
                }
            }
        }));
    }
    kick_main_queue(&mut sess, &main_q);
    let marker = main_marker(&mut sess);
    let ui = UiThread {
        main: Some(main_q),
        marker,
        gles: gles.clone(),
        game,
        screen_modes,
        vm: vm.clone(),
        flags: flags.clone(),
        cfg: cfg.clone(),
        platform: sess.platform.clone(),
        main_pid: pid,
        st: st.clone(),
        rx: Mutex::new(Some(rx)),
        runloop: {
            let export = |n: &str| sess.linker.modules().iter().find_map(|m| m.export(n));
            export("CFRunLoopRunInMode")
                .zip(export("kCFRunLoopDefaultMode"))
                .and_then(|(f, m)| Some((f, sess.cpu.mem().read_u32(m).ok()?)))
        },
    };
    if let Err(e) = spawn_ui_thread(&mut sess, ui) {
        report.stop = e;
        return Ok(finish(sess, &gles, report, flags, cfg, &blocks));
    }
    while !flags.startup_done.load(Ordering::SeqCst) && !flags.done.load(Ordering::SeqCst) {
        if flags.fail.lock().unwrap_or_else(PoisonError::into_inner).is_some() {
            break;
        }
        sched::sleep(&mut sess.cpu, &st, 2_000_000);
    }

    // 3. `MainThread.nativeRun` -> `main` -> `UIApplicationMain`, on a big stack. It only returns when the app ends
    // (or the UI thread shut the scheduler down).
    if !flags.startup_done.load(Ordering::SeqCst) {
        report.stop = "start-up failed".into();
        return Ok(finish(sess, &gles, report, flags, cfg, &blocks));
    }
    let big = sess.cpu.mem().map_anywhere(MAIN_STACK, 4096)?;
    sess.cpu.set_sp(big + MAIN_STACK);
    sess.cpu.set_ticks(FOREVER);
    eprintln!("[play] MainThread.nativeRun starts");
    flags.launch_step();
    report.stop = match vm.run_stage(&mut sess.cpu, Stage::MainThreadRun) {
        Ok(()) => "MainThread.nativeRun returned".to_string(),
        Err(Throw::Stop(s)) => {
            let bt = sess.cpu.stop_regs().map(|r| backtrace_lines(&sess.linker, &sess.cpu, r)).unwrap_or_default();
            report.backtrace = bt;
            format!("MainThread.nativeRun: guest stopped: {s}")
        }
        Err(Throw::Exc(e)) => {
            format!("MainThread.nativeRun: Java exception {}", vm.exception_message(e).unwrap_or_default())
        }
    };
    if let Some(why) = flags.ui_stop.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        report.stop = why;
        report.backtrace.clear();
    }
    Ok(finish(sess, &gles, report, flags, cfg, &blocks))
}

/// What the Android UI thread needs (moved into its start routine).
struct UiThread {
    main: Option<Arc<crate::mainq::MainQueue>>,
    /// Tells when the main run loop has run everything queued before it (see [`main_marker`]).
    marker: Option<MainMarker>,
    /// Presents host frames while a dialog waits and the game draws nothing.
    gles: Gles,
    /// The game's on-screen controls (hide, walk, zoom, pause), when the hooks went in.
    game: Option<Arc<GameHooks>>,
    /// Gives the game a screen mode of the new window size (see `follow_window_aspect`).
    screen_modes: Option<ScreenModeCalls>,
    vm: Vm,
    flags: Arc<Flags>,
    cfg: PlayConfig,
    platform: bhpc_jni::Platform,
    main_pid: usize,
    st: bhpc_bionic::Shared,
    rx: Mutex<Option<Receiver<Event>>>,
    /// `CFRunLoopRunInMode` and the default run loop mode: the UI thread has a run loop of its own that nothing else
    /// spins, so the timers that touch handlers schedule (`performSelector:afterDelay:`, UIView animations) would
    /// never fire; the UI loop turns it once per iteration.
    runloop: Option<(u32, u32)>,
}

type StageError = (String, [u32; 16]);

fn run_stage(c: &mut Cpu, vm: &Vm, stage: Stage) -> Result<(), StageError> {
    c.set_ticks(FOREVER);
    eprintln!("[play] stage {stage:?} (bound: {})", vm.stage_bound(stage));
    match vm.run_stage(c, stage) {
        Ok(()) => Ok(()),
        Err(Throw::Stop(s)) => Err((format!("{stage:?}: guest stopped: {s}"), c.stop_regs().unwrap_or([0; 16]))),
        Err(Throw::Exc(e)) => {
            Err((format!("{stage:?}: Java exception {}", vm.exception_message(e).unwrap_or_default()), [0; 16]))
        }
    }
}

/// Stages of the Java-side start-up (`Phase::Runtime`).
const RUNTIME_STEPS: u32 = 6;
/// `nativeRun` plus the four lifecycle stages that follow it (`Phase::Launch`).
const LAUNCH_STEPS: u32 = 5;

impl Flags {
    /// One more step of the launch phase; the last one starts the wait for the first frame.
    fn launch_step(&self) {
        let n = self.launch_steps.fetch_add(1, Ordering::SeqCst) + 1;
        progress(Phase::Launch, n, LAUNCH_STEPS);
        if n == LAUNCH_STEPS {
            progress(Phase::Frame, 0, 0);
        }
    }
}

impl UiThread {
    /// Body of the UI thread: the Java start-up sequence, then looper, layout and input until the game ends.
    fn main(&self, c: &mut Cpu) -> Result<String, StageError> {
        let vm = &self.vm;
        run_stage(c, vm, Stage::LibraryInit)?;
        progress(Phase::Runtime, 1, RUNTIME_STEPS);
        let env = bhpc_jni::native_env(&self.platform);
        c.set_ticks(FOREVER);
        vm.set_native_env(c, &env).map_err(|e| (format!("setenv: {e:?}"), [0; 16]))?;
        vm.attach_main_looper(self.main_pid);
        for (i, s) in [
            Stage::ActivityCreate,
            Stage::ActivityStart,
            Stage::ActivityResume,
            Stage::RootGeometry,
            Stage::SurfaceInit,
        ]
        .into_iter()
        .enumerate()
        {
            run_stage(c, vm, s)?;
            progress(Phase::Runtime, i as u32 + 2, RUNTIME_STEPS);
        }
        self.flags.startup_done.store(true, Ordering::SeqCst);
        // These queue blocks on the main run loop, which starts once the main thread enters `nativeRun`.
        for s in [Stage::ContextValid, Stage::SurfaceChanged, Stage::WillEnterForeground, Stage::DidBecomeActive] {
            run_stage(c, vm, s)?;
            self.flags.launch_step();
        }
        if let Some(uri) = &self.cfg.open_uri {
            eprintln!("[play] handing the game {uri}");
            if let Err(e) = vm.handle_uri(c, uri) {
                eprintln!("[play] nativeHandleUri failed: {e:?}");
            }
        }
        let rx = self.rx.lock().unwrap_or_else(PoisonError::into_inner).take();
        let mut mapper = InputMapper::new(InputConfig {
            window: self.cfg.window,
            bindings: self.cfg.bindings,
            pad: bhpc_input::PadConfig {
                enabled: self.cfg.controller,
                tile_cursor: self.cfg.pad_tile_cursor,
                ..bhpc_input::PadConfig::default()
            },
            ..InputConfig::default()
        });
        let why = self.ui_loop(c, &mut mapper, rx.as_ref());
        if CLOSES.contains(&why.as_str()) {
            self.close_app(c);
        }
        Ok(why)
    }

    /// The host actions that steer the game's own controls: hide its buttons, zoom, pause.
    fn game_actions(
        &self,
        c: &mut Cpu,
        actions: &[HostAction],
        content: (f32, f32),
        pad_cursor: Option<(f32, f32)>,
        now_ms: i64,
        wheel: &mut Option<crate::gameui::WheelAnim>,
    ) {
        use crate::gameui::{
            message, plan_zoom, view_point, wheel_target, zoom_base, zoom_cycle, WheelAnim, ZoomPlan, PINCH_BEGAN,
        };
        // The controller zooms around its cursor (the middle of the picture while the cursor is hidden).
        let pad_at = pad_cursor.unwrap_or((content.0 / 2.0, content.1 / 2.0));
        let pad_at = (pad_at.0.round() as i32, pad_at.1.round() as i32);
        for a in actions {
            let Some(g) = &self.game else {
                if *a == HostAction::ToggleGameButtons {
                    self.say("GAME BUTTONS CANNOT BE HIDDEN");
                }
                continue;
            };
            let mem = c.mem().clone();
            match a {
                HostAction::ToggleGameButtons => {
                    let hidden = g.ui.toggle();
                    eprintln!("[play] game buttons {}", if hidden { "hidden" } else { "shown" });
                    self.say(message(hidden));
                }
                HostAction::ZoomIn | HostAction::ZoomOut | HostAction::ZoomCycle | HostAction::WheelZoom { .. } => {
                    let Some(z) = g.ui.zoom(&mem) else { continue };
                    // A running step counts from where it is heading, so quick presses and notches add up.
                    let from = zoom_base(wheel.as_ref(), z);
                    let (to, at) = match *a {
                        HostAction::ZoomIn => (wheel_target(from, 1), pad_at),
                        HostAction::ZoomOut => (wheel_target(from, -1), pad_at),
                        HostAction::WheelZoom { steps, at } => (wheel_target(from, steps), at),
                        _ => (zoom_cycle(from), pad_at),
                    };
                    let centre = g.ui.view_size(&mem).and_then(|v| view_point(at, content, v));
                    match plan_zoom(wheel, z, to, now_ms, centre, g.pinch.is_some()) {
                        ZoomPlan::Retarget => {}
                        ZoomPlan::Begin(centre) => {
                            self.pinch(c, g, PINCH_BEGAN, 1.0, centre);
                            // The pinch starts from whatever the game shows once it began.
                            let base = g.ui.zoom(&mem).unwrap_or(z);
                            *wheel = Some(WheelAnim { base, from: base, to, start_ms: now_ms, centre });
                        }
                        ZoomPlan::Write => {
                            g.ui.set_zoom(&mem, to);
                        }
                    }
                    if std::env::var_os("BHPC_TRACE_ZOOM").is_some() {
                        eprintln!("[zoom] {a:?} at {at:?} {centre:?}: {z:.4} -> {to}");
                    }
                }
                HostAction::OpenChat => {
                    // In a world and not paused (the pause menu would sit over the chat).
                    let (Some((imp, sel)), Some(view)) = (g.chat, g.ui.game_view()) else { continue };
                    if g.ui.world(&mem).is_none() || g.ui.pause_shown(&mem) {
                        continue;
                    }
                    let main = self.main.clone();
                    let run = move |c: &mut Cpu| {
                        c.set_ticks(FOREVER);
                        if let Err(e) = c.call(imp, &[view, sel]) {
                            eprintln!("[play] chat: guest stopped: {e:?}");
                        }
                    };
                    match main {
                        Some(q) => q.call(c, run),
                        None => run(c),
                    }
                }
                HostAction::Pause => {
                    let Some(world) = g.ui.world(&mem) else { continue };
                    let (imp, sel) = if g.ui.pause_shown(&mem) { g.resume } else { g.pause };
                    let main = self.main.clone();
                    let run = move |c: &mut Cpu| {
                        c.set_ticks(FOREVER);
                        if let Err(e) = c.call(imp, &[world, sel]) {
                            eprintln!("[play] pause: guest stopped: {e:?}");
                        }
                    };
                    match main {
                        Some(q) => q.call(c, run),
                        None => run(c),
                    }
                }
                _ => {}
            }
        }
    }

    /// One call of the game's pinch handler, on the main thread like the game's own touches.
    fn pinch(&self, c: &mut Cpu, g: &GameHooks, state: u32, factor: f32, centre: (f32, f32)) {
        let (Some((imp, sel)), Some(view)) = (g.pinch, g.ui.game_view()) else { return };
        let args = crate::gameui::pinch_args(view, sel, state, factor, centre);
        let run = move |c: &mut Cpu| {
            c.set_ticks(FOREVER);
            if let Err(e) = c.call(imp, &args) {
                eprintln!("[play] wheel zoom: guest stopped: {e:?}");
            }
        };
        match self.main.clone() {
            Some(q) => q.call(c, run),
            None => run(c),
        }
    }

    /// Advance a running wheel step by one frame (see `gameui::WheelAnim`).
    fn wheel_frame(&self, c: &mut Cpu, now_ms: i64, wheel: &mut Option<crate::gameui::WheelAnim>) {
        use crate::gameui::{PINCH_CHANGED, PINCH_ENDED};
        let (Some(g), Some(a)) = (&self.game, *wheel) else { return };
        let mem = c.mem().clone();
        if g.ui.zoom(&mem).is_none() {
            // The world is gone: end the pinch so the view does not stay in one.
            self.pinch(c, g, PINCH_ENDED, 1.0, a.centre);
            *wheel = None;
            return;
        }
        let (want, done) = a.at(now_ms);
        self.pinch(c, g, PINCH_CHANGED, a.factor(want), a.centre);
        let shown = g.ui.zoom(&mem).unwrap_or(want);
        if a.refused(want, shown) {
            // The game did not take the pinch: zoom around the centre instead.
            self.pinch(c, g, PINCH_ENDED, 1.0, a.centre);
            g.ui.set_zoom(&mem, a.to);
            *wheel = None;
        } else if done {
            self.pinch(c, g, PINCH_ENDED, a.factor(want), a.centre);
            if a.missed(g.ui.zoom(&mem).unwrap_or(a.to)) {
                // The game dropped or bent the pinch: the step still lands.
                g.ui.set_zoom(&mem, a.to);
            }
            *wheel = None;
        }
        if std::env::var_os("BHPC_TRACE_ZOOM").is_some() {
            eprintln!(
                "[zoom] step {:.4} -> {:.4}: want {want:.4}, shows {shown:.4}{}",
                a.from,
                a.to,
                if done { " (done)" } else { "" }
            );
        }
    }

    /// While the chat is open or a text field has the keyboard, the world's idle timer is held at zero (typing is
    /// no touch, and after a minute without one the game pauses itself). The pause menu closes the chat (the game
    /// leaves it open under the menu): its CLOSE button is pressed.
    fn close_chat_when_paused(&self, c: &mut Cpu) {
        let Some(g) = &self.game else { return };
        if self.vm.text_focus().is_some() || self.vm.chat_close_button().is_some() {
            g.ui.reset_idle(&c.mem().clone());
        }
        if !g.ui.take_paused() {
            return;
        }
        let Some((close, composing)) = self.vm.chat_close_button() else { return };
        let vm = self.vm.clone();
        let presses = if composing { 2 } else { 1 };
        let answered = crate::input::on_main(self.main.as_ref(), c, move |c| {
            (0..presses).try_for_each(|_| vm.press_button(c, close))
        });
        if let Err(e) = answered {
            eprintln!("[play] closing the chat: {e:?}");
        }
    }

    /// Show a short message over the game.
    fn say(&self, text: &str) {
        self.flags.pad_view.lock().unwrap_or_else(PoisonError::into_inner).message =
            Some((text.to_string(), Instant::now()));
    }

    /// Press a button of `alert` (`None`: the configured or first button, else the cancel button).
    fn answer(&self, c: &mut Cpu, alert: &bhpc_jni::Alert, pick: Option<i32>) {
        let pick = pick
            .or(self.cfg.alert_choice)
            .or_else(|| alert.buttons.first().map(|b| b.1))
            .or_else(|| alert.cancel.as_ref().map(|b| b.1))
            .unwrap_or(0);
        eprintln!(
            "[play] alert {:?}: {:?}; buttons {:?} cancel {:?} -> pressing {pick}",
            alert.title, alert.message, alert.buttons, alert.cancel
        );
        c.set_ticks(4_000_000_000);
        // The buttons' delegate callbacks are the game's UIKit code: they run on the main thread like a control's
        // events. The start-up alert's answer creates the `GameView`; on the UI thread it raced the main thread's
        // `initOpenGL` (see `mainq`).
        let (vm, alert) = (self.vm.clone(), alert.clone());
        let answered = crate::input::on_main(self.main.as_ref(), c, move |c| vm.alert_click(c, &alert, pick));
        if let Err(Throw::Stop(s)) = answered {
            let msg = format!("alert: guest stopped: {s}");
            *self.flags.fail.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((msg, c.stop_regs().unwrap_or([0; 16])));
        }
    }

    /// One turn of the UI thread without input: a short sleep, the main looper, the thread's run loop, layout.
    fn turn(&self, c: &mut Cpu) -> Result<(), String> {
        let vm = &self.vm;
        sched::sleep(c, &self.st, 4_000_000);
        // Runnables other threads posted to the Android main looper (`performSelectorOnMainThread` and friends).
        c.set_ticks(FOREVER);
        match vm.pump(c, vm.main_looper(), vm.now_ms()) {
            Ok(_) => {}
            Err(Throw::Stop(s)) => return Err(format!("main looper: guest stopped: {s}")),
            Err(Throw::Exc(e)) => {
                eprintln!("[play] main looper: Java exception {}", vm.exception_message(e).unwrap_or_default());
            }
        }
        if let Some((run, mode)) = self.runloop {
            // `CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, true)`: the double sits in r2:r3, the flag on the stack.
            c.set_ticks(FOREVER);
            if let Err(e) = c.call(run, &[mode, 0, 0, 0, 1]) {
                return Err(format!("UI run loop: guest stopped: {e:?}"));
            }
        }
        match vm.run_layout(c) {
            Ok(_) => {}
            Err(Throw::Stop(s)) => return Err(format!("layout: guest stopped: {s}")),
            Err(Throw::Exc(e)) => {
                eprintln!("[play] layout: Java exception {}", vm.exception_message(e).unwrap_or_default());
            }
        }
        Ok(())
    }

    /// Close the app the way Android closes an activity, so the game saves its preferences and the world: resign
    /// active, `onPause`, background, `onStop`, then terminate and `onDestroy`. The lifecycle natives queue their work
    /// on the main run loop; after each half the UI thread keeps turning until the main thread has run it (a
    /// marker queued behind it), plus a moment for savers on other threads, at most a few seconds. The game's
    /// `onDestroy` calls `exit(0)`, which ends the run.
    fn close_app(&self, c: &mut Cpu) {
        eprintln!("[play] closing: the game goes to the background and terminates");
        let begun = Instant::now();
        for half in [
            &[Stage::WillResignActive, Stage::ActivityPause, Stage::DidEnterBackground, Stage::ActivityStop][..],
            &[Stage::WillTerminate, Stage::ActivityDestroy][..],
        ] {
            for s in half {
                // `onDestroy` ends the process itself (`exit(0)`), after everything else ran.
                if let Err((e, _)) = run_stage(c, &self.vm, *s) {
                    eprintln!("[play] closing: {e} ({} ms)", begun.elapsed().as_millis());
                    return;
                }
            }
            if let Err(e) = self.settle(c) {
                eprintln!("[play] closing: {e}");
                return;
            }
        }
        eprintln!("[play] closed in {} ms", begun.elapsed().as_millis());
    }

    /// Turn until the main run loop has run what is queued now (at most [`SETTLE_MAX`]), then [`SETTLE_GRACE`] more.
    fn settle(&self, c: &mut Cpu) -> Result<(), String> {
        let begun = Instant::now();
        let reached = self.marker.as_ref().map(|m| m.queue(c));
        while begun.elapsed() < SETTLE_MAX {
            if reached.as_ref().is_some_and(|r| r()) {
                break;
            }
            if lock(&self.st).sched.failure().is_some() {
                return Err("a guest thread failed".into());
            }
            self.turn(c)?;
        }
        let ran = begun.elapsed();
        if reached.is_none_or(|r| !r()) {
            eprintln!("[play] closing: the main thread did not get to it in {} ms", ran.as_millis());
        }
        while begun.elapsed() < ran + SETTLE_GRACE {
            self.turn(c)?;
        }
        Ok(())
    }

    /// A dialog shown while the game presents no frames (the consent alert comes before its first frame) would never
    /// reach the screen: the main thread presents one for it every [`HOST_FRAME`].
    fn present_dialog(&self, c: &mut Cpu, frames: &mut (u64, Instant)) {
        let n = self.flags.swaps.load(Ordering::SeqCst);
        if n != frames.0 {
            *frames = (n, Instant::now());
        }
        let Some(main) = self.main.as_ref().filter(|_| frames.1.elapsed() >= HOST_FRAME && self.vm.dialog_shown())
        else {
            return;
        };
        let (gles, vm) = (self.gles.clone(), self.vm.clone());
        static WARNED: AtomicBool = AtomicBool::new(false);
        // The frame goes the way of the game's own (`VerdeActivity.swapBuffers`): overlay, counters, screenshots.
        let shown = main.call(c, move |_| gles.context_current() && vm.swap_buffers());
        if !shown && !WARNED.swap(true, Ordering::SeqCst) {
            eprintln!("[play] no GL context on the main thread yet: the dialog cannot be shown");
        }
        *frames = (self.flags.swaps.load(Ordering::SeqCst), Instant::now());
    }

    /// The fake services' messages for the user ("achievements are not available"), and the activities the game
    /// started for a result: the picture picker's file dialog runs on a thread, its answer goes to the game here.
    fn services_frame(&self, c: &mut Cpu, pickers: &crate::picker::Pickers) {
        let vm = &self.vm;
        for n in vm.take_notices() {
            eprintln!("[play] notice: {n}");
            self.say(&n);
        }
        for req in vm.take_activity_requests() {
            eprintln!("[play] activity for a result: {req:?}");
            let fixed = std::env::var_os("BHPC_PICK_IMAGE");
            let dialogs = crate::picker::candidates(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default());
            pickers.start(&req, Box::new(move || crate::picker::pick_image(fixed, &dialogs)));
        }
        for (code, pick) in pickers.poll() {
            let (result, uri) = crate::picker::result_of(&pick);
            eprintln!("[play] activity {code}: result {result}, {uri:?}");
            if let Some(m) = crate::picker::message(&pick) {
                self.say(m);
            }
            c.set_ticks(FOREVER);
            if let Err(e) = vm.activity_result(c, code, result, uri.as_deref()) {
                eprintln!("[play] activity result: {e:?}");
            }
        }
    }

    fn ui_loop(&self, c: &mut Cpu, mapper: &mut InputMapper, rx: Option<&Receiver<Event>>) -> String {
        let (vm, flags, cfg, st) = (&self.vm, &self.flags, &self.cfg, &self.st);
        let begun = Instant::now();
        let mut tracker = InputTracker::default();
        let mut focused: Option<bhpc_jni::Obj> = None;
        let mut dialog_keys = crate::input::DialogInput::default();
        // The game's frame count and when it last changed: a dialog with no game frames is presented by the host.
        let mut frames = (flags.swaps.load(Ordering::SeqCst), Instant::now());
        let pickers = crate::picker::Pickers::default();
        // The wheel step in progress (the game's own pinch, a few frames long).
        let mut wheel: Option<crate::gameui::WheelAnim> = None;
        // Window pixels per logical pixel, for clicks on the mod UI (its picture is in window pixels).
        let mut mods_scale = 0.0f32;
        loop {
            if flags.done.load(Ordering::SeqCst) {
                return "window closed".into();
            }
            if flags.script_quit.load(Ordering::SeqCst) {
                return "script quit".into();
            }
            if cfg.exit_after_shot && flags.shot_taken.load(Ordering::SeqCst) {
                return "screenshot taken".into();
            }
            if cfg.seconds.is_some_and(|s| begun.elapsed() >= Duration::from_secs(s)) {
                return format!("time limit reached after {} swaps", flags.swaps.load(Ordering::SeqCst));
            }
            if let Some(f) = lock(st).sched.failure() {
                return format!("a guest thread failed: {f}");
            }
            // Touch times are `SystemClock.uptimeMillis`; UIKit compares them with `systemUptime`, the guest's
            // monotonic clock, to expire taps.
            let now = {
                let (secs, nanos) = lock(st).clock.monotonic();
                secs * 1000 + i64::from(nanos / 1_000_000)
            };
            if let Err(why) = self.turn(c) {
                return why;
            }
            for alert in vm.take_alerts() {
                if alert.field.is_some() {
                    // An alert with an input line waits for the typed text; Return answers it with the first button,
                    // Escape with the cancel button.
                    eprintln!("[play] alert {:?} with a text field: waiting for text input", alert.title);
                } else if cfg.auto_alerts {
                    self.answer(c, &alert, None);
                    if flags.fail.lock().unwrap_or_else(PoisonError::into_inner).is_some() {
                        return "alert: guest stopped".into();
                    }
                } else {
                    // Its dialog is up (`render::dialog`); the user's pick answers it below.
                    eprintln!(
                        "[play] alert {:?}: {:?}; buttons {:?} cancel {:?}: shown",
                        alert.title, alert.message, alert.buttons, alert.cancel
                    );
                }
            }
            self.present_dialog(c, &mut frames);
            self.services_frame(c, &pickers);
            if flags.dump_views.swap(false, Ordering::SeqCst) {
                eprintln!("[views]\n{}", vm.dump_views());
            }
            let focus = vm.text_focus();
            if focus != focused {
                focused = focus;
                let mut sink = JniSink {
                    vm,
                    cpu: c,
                    actions: Vec::new(),
                    errors: Vec::new(),
                    returned: false,
                    main: self.main.clone(),
                    game: self.game.as_ref().map(|g| g.ui.clone()),
                };
                match focus {
                    Some(f) => {
                        let (handle, text) = (vm.get_field(f, "mObject").as_int(), vm.text_of(f));
                        mapper.set_text_focus(Some((handle, &text)), &mut sink);
                    }
                    None => mapper.set_text_focus(None, &mut sink),
                }
            }
            let mut sink = JniSink {
                vm,
                cpu: c,
                actions: Vec::new(),
                errors: Vec::new(),
                returned: false,
                main: self.main.clone(),
                game: self.game.as_ref().map(|g| g.ui.clone()),
            };
            let mut escape = false;
            // The game announced (or changed) its content size: touches map into it.
            if let Some((w, h)) = vm.content_size().filter(|s| Some(*s) != mapper.geometry().content) {
                mapper.set_content(w, h);
            }
            if let Some(g) = &self.game {
                let content = mapper.geometry().content.unwrap_or((cfg.window.0 as f32, cfg.window.1 as f32));
                let mem = sink.cpu.mem().clone();
                let dpad = g.ui.dpad(&mem).and_then(|(size, right)| {
                    crate::gameui::dpad_centre(size, content, right, crate::gameui::DPAD_INSET)
                });
                mapper.pad_mut().set_grid(g.ui.grid(&mem, content));
                mapper.set_world_zoom(g.ui.zoom(&mem).is_some());
                mapper.pad_mut().set_dpad(dpad);
            }
            let pending: Vec<Event> = std::iter::from_fn(|| rx.and_then(|r| r.try_recv().ok())).collect();
            if mods_scale == 0.0 {
                mods_scale = vm.screen_size().0.max(1) as f32 / cfg.window.0.max(1) as f32;
            }
            for ev in crate::input::coalesce_moves(pending) {
                let mut back = false;
                if let Event::Key { key, pressed: true, mods, .. } = &ev {
                    let held = bhpc_input::Mods { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt };
                    back = cfg.bindings.back.fires(Input::Key(host_key(*key)), held);
                    escape |= back;
                }
                // The mod UI (F9 panel, join prompt) is modal while it shows.
                if let Event::Resized(m) = &ev {
                    mods_scale = m.drawable.0 as f32 / m.logical.0.max(1) as f32;
                }
                let mods = flags.mods.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if let Some(m) = mods {
                    use bhpc_mods::ui::UiInput;
                    let input = match &ev {
                        Event::Key { key, pressed: true, .. } => UiInput::Key(key.0),
                        Event::MouseButton { button: bhpc_app::MouseButton::Left, pressed: true, x, y } => {
                            UiInput::Click(x * mods_scale, y * mods_scale)
                        }
                        Event::Resized(_) | Event::Focus(_) | Event::Visible(_) | Event::Quit => UiInput::Other,
                        _ => UiInput::Other,
                    };
                    let passive = matches!(ev, Event::Resized(_) | Event::Focus(_) | Event::Visible(_) | Event::Quit);
                    let (took, cmd) = {
                        let view = m.view.lock().unwrap_or_else(PoisonError::into_inner);
                        m.ui.lock().unwrap_or_else(PoisonError::into_inner).input(input, &view)
                    };
                    match cmd {
                        // The clipboard belongs to this thread (SDL) or the launcher (`@copy`).
                        Some(bhpc_mods::view::UiCmd::Copy(text)) if cfg.embed.is_some() => {
                            println!("{}", bhpc_launcher::embed::copy_line(&text));
                        }
                        Some(bhpc_mods::view::UiCmd::Copy(text)) => {
                            *flags.clipboard.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
                        }
                        Some(cmd) => m.post(cmd),
                        None => {}
                    }
                    if took && !passive {
                        m.serial.fetch_add(1, Ordering::SeqCst);
                        continue;
                    }
                }
                // A shown dialog is modal: its keys and controller buttons do not reach the game.
                if let Some(nav) = dialog_keys.filter(&ev, vm.dialog_alert().map(|d| d.field.is_some()), back) {
                    if let Some(nav) = nav {
                        vm.dialog_nav(nav);
                    }
                    continue;
                }
                if let Event::Resized(m) = &ev {
                    // The display of the game follows the window; `SurfaceChanged` below tells the game.
                    vm.set_screen_size(m.drawable.0 as i32, m.drawable.1 as i32);
                    eprintln!("[play] window {:?}, drawable {:?}", m.logical, m.drawable);
                    // The game's screen takes the new size before the surface change reaches it.
                    if let Some(calls) = self.screen_modes.clone() {
                        if let Err(e) = crate::input::on_main(self.main.as_ref(), sink.cpu, move |c| calls.apply(c)) {
                            eprintln!("[play] the game's screen keeps its size: {e}");
                        }
                    }
                }
                if let Event::Wheel { dx, dy } = &ev {
                    if std::env::var_os("BHPC_TRACE_ZOOM").is_some() {
                        eprintln!("[zoom] host wheel dx {dx} dy {dy} at {now} ms");
                    }
                }
                if let Some(h) = tracker.map(&ev) {
                    mapper.handle(h, now, &mut sink);
                }
            }
            mapper.tick(now, &mut sink);
            *flags.geometry.lock().unwrap_or_else(PoisonError::into_inner) = Some(mapper.geometry());
            for e in sink.errors.drain(..) {
                eprintln!("[play] input delivery: {e}");
            }
            let (returned, actions) = (sink.returned, std::mem::take(&mut sink.actions));
            if actions.contains(&HostAction::Quit) {
                return "quit requested".into();
            }
            if actions.contains(&HostAction::ToggleFullscreen) && cfg.embed.is_none() {
                flags.toggle_fullscreen.store(true, Ordering::SeqCst);
            }
            let content = mapper.geometry().content.unwrap_or((cfg.window.0 as f32, cfg.window.1 as f32));
            self.game_actions(c, &actions, content, mapper.pad().cursor(), now, &mut wheel);
            self.wheel_frame(c, now, &mut wheel);
            self.close_chat_when_paused(c);
            {
                let mut v = flags.pad_view.lock().unwrap_or_else(PoisonError::into_inner);
                v.cursor = mapper.pad().cursor();
                v.tile = mapper.pad().tile_size();
                v.content = mapper.geometry().content.unwrap_or((cfg.window.0 as f32, cfg.window.1 as f32));
            }
            vm.set_dialog_caret(mapper.text_caret());
            // An input dialog: Return in its field presses the focused (first) button, Escape the cancel button.
            if vm.dialog_alert().is_some_and(|d| d.field.is_some()) {
                if returned {
                    vm.dialog_nav(bhpc_jni::DialogNav::Accept);
                } else if escape {
                    vm.dialog_nav(bhpc_jni::DialogNav::Cancel);
                }
            }
            // A click on a dialog button, or a key or controller button that pressed one.
            if let Some((alert, pick)) = vm.take_dialog_pick() {
                eprintln!(
                    "[play] alert {:?} answered with {pick}: {:?}",
                    alert.title,
                    alert.field.map(|f| vm.text_of(f))
                );
                self.answer(c, &alert, Some(pick));
                if flags.fail.lock().unwrap_or_else(PoisonError::into_inner).is_some() {
                    return "alert: guest stopped".into();
                }
                if alert.field.is_none() {
                    continue;
                }
                let mut sink = JniSink {
                    vm,
                    cpu: c,
                    actions: Vec::new(),
                    errors: Vec::new(),
                    returned: false,
                    main: self.main.clone(),
                    game: self.game.as_ref().map(|g| g.ui.clone()),
                };
                mapper.set_text_focus(None, &mut sink);
                focused = None;
            }
        }
    }
}

/// How the UI loop ends when the user closes the game (the window, the launcher's `quit` or end of input, a script's
/// `quit`): these run [`UiThread::close_app`] first.
const CLOSES: [&str; 3] = ["window closed", "script quit", "quit requested"];
/// How often a dialog is presented while the game draws no frames.
const HOST_FRAME: Duration = Duration::from_millis(100);
/// The longest wait for the main run loop while closing, per half of the sequence.
const SETTLE_MAX: Duration = Duration::from_secs(4);
/// Extra time after the main run loop got there (savers on other threads).
const SETTLE_GRACE: Duration = Duration::from_millis(400);

/// A job for the main run loop that only counts its runs: `dispatch_async_f(&_dispatch_main_q, 0, marker)`. The main
/// queue runs in order, so once it ran, the main thread has run everything queued before it.
struct MainMarker {
    async_f: u32,
    main_q: u32,
    thunk: u32,
    runs: Arc<AtomicU64>,
}

impl MainMarker {
    /// Queue the marker; the result tells whether it has run.
    fn queue(&self, c: &mut Cpu) -> impl Fn() -> bool {
        let (runs, want) = (self.runs.clone(), self.runs.load(Ordering::SeqCst) + 1);
        c.set_ticks(FOREVER);
        if let Err(e) = c.call(self.async_f, &[self.main_q, 0, self.thunk]) {
            eprintln!("[play] main queue marker: guest stopped: {e}");
        }
        move || runs.load(Ordering::SeqCst) >= want
    }
}

fn main_marker(sess: &mut Session) -> Option<MainMarker> {
    let export = |n: &str| sess.linker.modules().iter().find_map(|m| m.export(n));
    let (async_f, main_q) = (export("dispatch_async_f")?, export("_dispatch_main_q")?);
    let runs = Arc::new(AtomicU64::new(0));
    let r = runs.clone();
    let f: bhpc_cpu::SharedThunkFn = Arc::new(move |_: &mut Cpu| {
        r.fetch_add(1, Ordering::SeqCst);
    });
    let thunk = sess.cpu.register_shared_thunk("bhpc:main_queue_marker", f).ok()?;
    Some(MainMarker { async_f, main_q, thunk, runs })
}

/// While the main thread presents no frames (start-up), a job waiting in `queue` for 20 ms asks libdispatch to run
/// the queue from the main run loop: `dispatch_async_f(&_dispatch_main_q, 0, drain)`. Without libdispatch the poster
/// keeps waiting and finally runs the job itself, as before.
fn kick_main_queue(sess: &mut Session, queue: &Arc<crate::mainq::MainQueue>) {
    let export = |n: &str| sess.linker.modules().iter().find_map(|m| m.export(n));
    let (Some(async_f), Some(main_q)) = (export("dispatch_async_f"), export("_dispatch_main_q")) else { return };
    let q = queue.clone();
    let drain: bhpc_cpu::SharedThunkFn = Arc::new(move |c: &mut Cpu| q.drain(c));
    let Ok(drain) = sess.cpu.register_shared_thunk("bhpc:main_queue_drain", drain) else { return };
    queue.set_kick(20, move |c| {
        if let Err(e) = c.call(async_f, &[main_q, 0, drain]) {
            eprintln!("[play] main queue kick: guest stopped: {e}");
        }
    });
}

/// Create the guest thread of the Android UI thread (a thunk as the start routine, a small stack).
fn spawn_ui_thread(sess: &mut Session, ui: UiThread) -> Result<(), String> {
    let (flags, st) = (ui.flags.clone(), ui.st.clone());
    let entry = sess
        .cpu
        .register_shared_thunk(
            "bhpc:ui_thread",
            Arc::new(move |c: &mut Cpu| {
                match ui.main(c) {
                    Ok(why) => {
                        *ui.flags.ui_stop.lock().unwrap_or_else(PoisonError::into_inner) = Some(why);
                    }
                    Err((msg, regs)) => {
                        *ui.flags.fail.lock().unwrap_or_else(PoisonError::into_inner) = Some((msg, regs));
                    }
                }
                // Wake the main thread out of its run loop so `nativeRun` returns and the process can end.
                let sched = lock(&ui.st).sched.clone();
                sched.shutdown();
                c.ret_u32(0);
            }),
        )
        .map_err(|e| e.to_string())?;
    let _ = (flags, st);
    let create = bhpc_bionic::resolve(&mut sess.cpu, &sess.st, "pthread_create").ok_or("no pthread_create")?;
    let area = lock(&sess.st).heap.calloc(1, 64);
    if area == 0 {
        return Err("guest heap exhausted".into());
    }
    sess.cpu.mem().write_u32(area + 8 + 8, 1 << 20).map_err(|e| e.to_string())?;
    sess.cpu.set_ticks(FOREVER);
    match sess.cpu.call(create, &[area, area + 8, entry, 0]) {
        Ok(r) if r.r0 == 0 => Ok(()),
        Ok(r) => Err(format!("pthread_create failed: {}", r.r0)),
        Err(s) => Err(format!("pthread_create stopped: {s}")),
    }
}

fn finish(
    sess: Session,
    gles: &Gles,
    mut r: PlayReport,
    flags: &Arc<Flags>,
    cfg: &PlayConfig,
    blocks: &BlockLog,
) -> PlayReport {
    let sym = &cfg.symbolize;
    if let Some(h) = DEV_HOOKS.get() {
        // With BHPC_THUNK_STATS=1: host time per call of each hook thunk (an after hook includes the original).
        for t in sess.cpu.table().stats().iter().filter(|t| t.name.starts_with("bhpc:objc:") && t.calls > 0) {
            eprintln!(
                "[dev-hook] {}: {} calls, {:.0} ns per call in the thunk",
                t.name,
                t.calls,
                t.nanos as f64 / t.calls as f64
            );
        }
        for st in h.stats() {
            eprintln!("[dev-hook] {} {}: {} calls, {} panics {}", st.name, st.state, st.calls, st.panics, st.note);
        }
    }
    if std::env::var_os("BHPC_THUNK_STATS").is_some() {
        for t in sess.cpu.table().stats().iter().take(60) {
            let free = if t.free { " (gate-free)" } else { "" };
            eprintln!("[thunks] {:>10} calls {:>10.1} ms  {}{free}", t.calls, t.nanos as f64 / 1e6, t.name);
        }
    }
    if let Some(p) = &cfg.jni_report {
        let _ = std::fs::write(p, sess.jni.vm.report().render());
    }
    for (tid, (name, regs)) in blocks.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        eprintln!("[blocked] guest thread {tid}: last blocking call {name}");
        for l in backtrace_lines(&sess.linker, &sess.cpu, *regs).iter().take(40) {
            eprintln!("    {l}");
        }
    }
    for a in sym {
        eprintln!("[sym] {}", crate::apkrun::symbolize(&sess.linker, &sess.cpu, *a));
    }
    r.swaps = flags.swaps.load(Ordering::SeqCst);
    {
        let times = flags.swap_times.lock().unwrap_or_else(PoisonError::into_inner);
        let marks = flags.marks.lock().unwrap_or_else(PoisonError::into_inner);
        for l in framestats::report(&times, &marks) {
            eprintln!("{l}");
        }
    }
    if let Some((msg, regs)) = flags.fail.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        r.backtrace = backtrace_lines(&sess.linker, &sess.cpu, regs);
        if r.stop.is_empty() || r.stop == "window closed" {
            r.stop = msg;
        }
    }
    r.java_stubs = sess
        .jni
        .vm
        .report()
        .stubs()
        .filter(|m| m.calls > 0)
        .map(|m| format!("{}.{}{} x{}", m.class, m.name, m.sig, m.calls))
        .collect();
    let stats = gles.stats();
    eprintln!("[play] GL calls handled: {}", stats.calls);
    r.gl_unsupported = stats.unsupported;
    r.unimplemented_hits = bhpc_bionic::lock(&sess.st).unimplemented_hits.clone();
    eprintln!("[play] imports still bound to traps: {}", bhpc_bionic::lock(&sess.st).unimplemented_bound.join(" "));
    r.top_thunks = bhpc_bionic::histogram(&sess.st).into_iter().filter(|(_, c)| *c > 0).collect();
    sess.shutdown();
    r
}

/// Patch the first two instructions of the ARM function `symbol` (exported by any loaded library) with a jump to a
/// host hook; the displaced instructions run in a trampoline, so the original continues unchanged. `hook` sees the
/// guest registers on entry.
fn hook_function(
    sess: &mut Session,
    symbol: &str,
    hook: impl Fn(&mut Cpu) + Send + Sync + 'static,
) -> Result<(), String> {
    // `libX.so+0x1234` names a function by its ELF address inside a module (no export needed).
    let at_offset = symbol.split_once("+0x").and_then(|(module, off)| {
        let m = sess.linker.module(module)?;
        Some(m.bias.wrapping_add(u32::from_str_radix(off, 16).ok()?))
    });
    let entry = at_offset
        .or_else(|| sess.linker.modules().iter().find_map(|m| m.export(symbol)))
        .ok_or(format!("no {symbol} export"))?;
    hook_at(sess, entry, symbol, hook)
}

/// Run `hook` before the ARM function at `entry` (its first two instructions move to a trampoline).
fn hook_at(
    sess: &mut Session,
    entry: u32,
    symbol: &str,
    hook: impl Fn(&mut Cpu) + Send + Sync + 'static,
) -> Result<(), String> {
    hook_or_skip(sess, entry, symbol, move |c| {
        hook(c);
        None
    })
}

/// Like [`hook_at`], but `decide` may answer for the function: `Some(r0)` returns that to the caller at once
/// without running the function.
fn hook_or_skip(
    sess: &mut Session,
    entry: u32,
    symbol: &str,
    decide: impl Fn(&mut Cpu) -> Option<u32> + Send + Sync + 'static,
) -> Result<(), String> {
    if entry & 1 != 0 {
        return Err(format!("{symbol} is Thumb code"));
    }
    let mem = sess.cpu.mem().clone();
    let orig = mem.read_bytes(entry, 8).map_err(|e| e.to_string())?;
    // The displaced instructions run elsewhere: they must not read the pc.
    for w in orig.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) {
        let (class, rn, imm) = ((w >> 26) & 3, (w >> 16) & 0xf, w & (1 << 25) != 0);
        if rn == 15 || (class == 0 && !imm && w & 0xf == 15) {
            return Err(format!("{symbol} starts with a pc-relative instruction ({w:#010x})"));
        }
    }
    let tramp = lock(&sess.st).heap.alloc(16);
    let mut code = orig.clone();
    code.extend_from_slice(&0xE51F_F004u32.to_le_bytes()); // ldr pc, [pc, #-4]
    code.extend_from_slice(&(entry + 8).to_le_bytes());
    mem.write_bytes(tramp, &code).map_err(|e| e.to_string())?;
    let thunk = sess
        .cpu
        .register_shared_thunk(
            &format!("bhpc:trace:{symbol}"),
            Arc::new(move |c: &mut Cpu| match decide(c) {
                // Not continuing anywhere returns to the caller.
                Some(r0) => c.set_reg(0, r0),
                None => c.tail_call(tramp),
            }),
        )
        .map_err(|e| e.to_string())?;
    let mut patch = 0xE51F_F004u32.to_le_bytes().to_vec();
    patch.extend_from_slice(&thunk.to_le_bytes());
    mem.write_bytes(entry, &patch).map_err(|e| e.to_string())?;
    sess.cpu.clear_cache();
    Ok(())
}

/// How the host gives the game a new screen mode when the window changes size: `+[UIScreen mainScreen]`,
/// `+[UIScreenMode emulatedMode:]` and `-[UIScreen setCurrentMode:]` (class, implementation, selector name).
#[derive(Debug, Clone)]
struct ScreenModeCalls {
    screen_class: u32,
    main_screen: (u32, u32),
    mode_class: u32,
    emulated: (u32, u32),
    set_mode: (u32, u32),
    /// `-[EAGLView layoutSubviews]` and the game's GL view (seen in `setFramebuffer`, 0 before): the layout drops the
    /// framebuffer, and the next frame makes one of the new size and calls `-[GameView resize:insets:]`.
    layout: (u32, u32),
    view: Arc<AtomicU32>,
}

impl ScreenModeCalls {
    /// `[[UIScreen mainScreen] setCurrentMode:[UIScreenMode emulatedMode:12]]`: a native mode for the current screen
    /// (`aspect::content_size` at the first scale). `-[UIScreen _applyMode]` publishes it as the new content size (`setContentSize`); the game's
    /// `EAGLView` then makes a framebuffer of that size at its next layout and `-[GameView resize:insets:]` re-lays
    /// the game out. Runs on the guest main thread.
    fn apply(&self, c: &mut Cpu) -> Result<(), String> {
        c.set_ticks(FOREVER);
        let err = |e| format!("{e:?}");
        let screen = c.call(self.main_screen.0, &[self.screen_class, self.main_screen.1]).map_err(err)?.r0;
        let mode =
            c.call(self.emulated.0, &[self.mode_class, self.emulated.1, crate::aspect::MODE_NATIVE]).map_err(err)?.r0;
        if screen == 0 || mode == 0 {
            return Err(format!("no screen ({screen:#x}) or mode ({mode:#x})"));
        }
        c.call(self.set_mode.0, &[screen, self.set_mode.1, mode]).map_err(err)?;
        let view = self.view.load(Ordering::SeqCst);
        if view != 0 {
            c.call(self.layout.0, &[view, self.layout.1]).map_err(err)?;
        }
        Ok(())
    }
}

/// The game's screen is the window's size and aspect (see `aspect`): `emulatedMode:` gives the native mode instead
/// of the best emulated iOS device, and the native mode's scale is [`crate::aspect::ui_scale`] of the display at
/// `zoom`.
fn follow_window_aspect(
    sess: &mut Session,
    zoom: crate::aspect::GameZoom,
    fine: f32,
) -> Result<ScreenModeCalls, String> {
    use crate::aspect::{called_from_native_mode, screen_mode, ui_scale};
    use crate::fpsunlock::find_method;
    let mem = sess.cpu.mem().clone();
    let class = |name: &str| {
        sess.linker
            .modules()
            .iter()
            .find_map(|m| m.export(&format!("OBJC_CLASS_$_{name}")))
            .ok_or(format!("no class {name}"))
    };
    let (mode, native) = (class("UIScreenMode")?, class("UINativeScreenMode")?);
    let emulated = find_method(&mem, mode, "emulatedMode:", true).ok_or("no +[UIScreenMode emulatedMode:]")?;
    let init = find_method(&mem, mode, "initWithScale:", false).ok_or("no -[UIScreenMode initWithScale:]")?;
    let native_mode = find_method(&mem, native, "nativeMode:", true).ok_or("no +[UINativeScreenMode nativeMode:]")?;
    let screen_class = class("UIScreen")?;
    let call = |cls: u32, sel: &str, meta: bool| {
        method_and_name(&mem, cls, sel, meta).ok_or(format!("no {}[{sel}]", if meta { '+' } else { '-' }))
    };
    let eagl = class("EAGLView")?;
    let set_framebuffer = find_method(&mem, eagl, "setFramebuffer", false).ok_or("no -[EAGLView setFramebuffer]")?;
    let view = Arc::new(AtomicU32::new(0));
    let calls = ScreenModeCalls {
        layout: call(eagl, "layoutSubviews", false)?,
        view: view.clone(),
        screen_class,
        main_screen: call(screen_class, "mainScreen", true)?,
        mode_class: mode,
        emulated: call(mode, "emulatedMode:", true)?,
        set_mode: call(screen_class, "setCurrentMode:", false)?,
    };
    hook_at(sess, emulated, "+[UIScreenMode emulatedMode:]", |c| {
        let m = screen_mode(c.reg(2));
        c.set_reg(2, m);
    })?;
    hook_at(sess, set_framebuffer, "-[EAGLView setFramebuffer]", move |c| view.store(c.reg(0), Ordering::SeqCst))?;
    let native_size = find_method(&mem, native, "size", false).ok_or("no -[UINativeScreenMode size]")?;
    let vm = sess.jni.vm.clone();
    // The game reads its `PIXEL_SCALE` from the screen once at start and lays its views out with it: later modes
    // keep that scale (a new scale left the HUD where the old one put it) and get a size for it.
    let first = Arc::new(Mutex::new(None::<f32>));
    let scale = first.clone();
    hook_at(sess, init, "-[UIScreenMode initWithScale:]", move |c| {
        if called_from_native_mode(c.reg(14), native_mode) {
            let (w, h) = vm.screen_size();
            let s =
                *scale.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| ui_scale(w, h, zoom, fine));
            eprintln!("[play] screen {w}x{h} pixels, {s} pixels per point");
            c.set_reg(2, s.to_bits());
        }
    })?;
    // `-[UINativeScreenMode size]` (the struct is returned through r0): the display's pixels in portrait order, for
    // the first scale (see `aspect::content_size`).
    let vm = sess.jni.vm.clone();
    hook_or_skip(sess, native_size, "-[UINativeScreenMode size]", move |c| {
        let s = (*first.lock().unwrap_or_else(PoisonError::into_inner))?;
        let (w, h) = vm.screen_size();
        let (cw, ch) = crate::aspect::content_size(w, h, s, zoom, fine);
        let out = c.reg(0);
        let ok = c.mem().write_u32(out, cw.min(ch).to_bits()).is_ok()
            && c.mem().write_u32(out + 4, cw.max(ch).to_bits()).is_ok();
        ok.then_some(out)
    })?;
    sess.cpu.clear_cache();
    Ok(calls)
}

/// Implementation and selector-name pointer of a method of a not yet realised class (see `fpsunlock::methods`).
fn method_and_name(mem: &bhpc_mem::GuestMemory, class: u32, sel: &str, meta: bool) -> Option<(u32, u32)> {
    let c = if meta { mem.read_u32(class).ok()? } else { class };
    let list = mem.read_u32((mem.read_u32(c + 16).ok()? & !3) + 20).ok()?;
    let i = crate::fpsunlock::methods(mem, class, meta).iter().position(|(n, _)| n == sel)? as u32;
    let at = list + 8 + i * (mem.read_u32(list).ok()? & !3);
    Some((mem.read_u32(at + 8).ok()?, mem.read_u32(at).ok()?))
}

/// The "Unlock 30 FPS cap" option (see `fpsunlock`): the display link's refresh rate constants and its
/// `setFrameInterval:` argument. Returns the number of constants patched.
fn unlock_fps(sess: &mut Session) -> Result<usize, String> {
    use crate::fpsunlock::{methods, patch_refresh_rate, unlocked_interval, UNLOCKED_HZ};
    let class = sess
        .linker
        .modules()
        .iter()
        .find_map(|m| m.export("OBJC_CLASS_$_CADisplayLink"))
        .ok_or("no CADisplayLink class")?;
    let list = methods(sess.cpu.mem(), class, false);
    let set = list.iter().find(|(n, _)| n == "setFrameInterval:").map(|m| m.1).ok_or("no setFrameInterval:")?;
    let imps: Vec<u32> = list.iter().map(|m| m.1).collect();
    let patched = patch_refresh_rate(sess.cpu.mem(), &imps, UNLOCKED_HZ);
    if patched == 0 {
        return Err("no refresh-rate constant in CADisplayLink".into());
    }
    hook_at(sess, set, "-[CADisplayLink setFrameInterval:]", |c| {
        let n = unlocked_interval(c.reg(2) as i32);
        c.set_reg(2, n as u32);
    })?;
    sess.cpu.clear_cache();
    Ok(patched)
}

/// The game's on-screen controls as the host steers them (see `gameui`), and the pause button's two actions
/// (implementation and selector name, called with the `World`).
struct GameHooks {
    ui: Arc<crate::gameui::GameUi>,
    pause: (u32, u32),
    resume: (u32, u32),
    /// `-[GameView showChatUI]` (implementation, selector name): the chat key opens the game's own chat.
    chat: Option<(u32, u32)>,
    /// `-[GameView pinchGesture:velocity:state:center:numberOfTouches:]` (implementation, selector name): the wheel
    /// zooms through it so the game keeps the point under the cursor in place.
    pinch: Option<(u32, u32)>,
}

/// The methods "Hide game buttons" stops while the buttons are hidden: class, selector and whether it is the
/// D-pad's (hidden on its own with `--hide-dpad`; its touch handling still takes the controller's walking finger).
/// While stopped a method answers `NO` (the touch questions) or does nothing (the renderers).
const HIDDEN_METHODS: [(&str, &str, bool); 9] = [
    ("WorldUI", "render:translation:pinchScale:paused:", false),
    ("WorldUI", "touchIsInUI:", false),
    ("WorldUI", "tap:", false),
    ("WorldUI", "startTouch:tapCount:index:", false),
    ("WorldUI", "startTouch:tapCount:paused:index:", false),
    ("DPad", "render:", true),
    ("DPad", "touchIsInUI:", true),
    ("DPad", "touchIsInViewAtAll:", true),
    ("DPad", "startTouch:tapCount:index:", true),
];

/// Hook the game's controls (see `gameui`): remember the `GameView` it renders, stop the buttons' rendering and
/// touch handling while they are hidden, and find the pause actions. `hide_dpad` keeps the D-pad hidden
/// for the whole run (`GameUi::dpad_hidden`).
fn install_game_ui(sess: &mut Session, hide_dpad: bool) -> Result<Arc<GameHooks>, String> {
    use crate::fpsunlock::methods;
    let mem = sess.cpu.mem().clone();
    let export = |name: &str| sess.linker.modules().iter().find_map(|m| m.export(name));
    let ivars = crate::gameui::Ivars::read(&mem, &export)?;
    let class = |name: &str| export(&format!("OBJC_CLASS_$_{name}")).ok_or(format!("no class {name}"));
    // Implementation and selector-name pointer of a method (the runtime is not up yet: names are C strings).
    let method = |cls: &str, sel: &str| -> Result<(u32, u32), String> {
        let c = class(cls)?;
        let ro = mem.read_u32(c + 16).map_err(|e| e.to_string())? & !3;
        let list = mem.read_u32(ro + 20).map_err(|e| e.to_string())?;
        let found = methods(&mem, c, false).into_iter().position(|(n, _)| n == sel);
        let i = found.ok_or(format!("no -[{cls} {sel}]"))? as u32;
        let size = mem.read_u32(list).map_err(|e| e.to_string())? & !3;
        let at = list + 8 + i * size;
        Ok((mem.read_u32(at + 8).map_err(|e| e.to_string())?, mem.read_u32(at).map_err(|e| e.to_string())?))
    };
    let render = method("GameView", "render:")?.0;
    let hidden: Vec<(u32, &str, &str, bool)> = HIDDEN_METHODS
        .iter()
        .map(|(cls, sel, dpad)| method(cls, sel).map(|(imp, _)| (imp, *cls, *sel, *dpad)))
        .collect::<Result<_, _>>()?;
    let (pause, resume) = (method("World", "pauseButtonTapped")?, method("World", "pauseResumeButtonTapped")?);
    let chat = method("GameView", "showChatUI").inspect_err(|e| eprintln!("[play] no chat key: {e}")).ok();
    let pinch = method("GameView", crate::gameui::PINCH_SELECTOR)
        .inspect_err(|e| eprintln!("[play] the wheel zooms around the centre: {e}"))
        .ok();
    let idle = export("OBJC_IVAR_$_World.pauseIdleTimer").and_then(|at| mem.read_u32(at).ok()).unwrap_or(0);
    if idle == 0 {
        eprintln!("[play] no World.pauseIdleTimer: typing in the chat may let the game pause itself");
    }
    let ui = Arc::new(crate::gameui::GameUi::new(ivars).dpad_hidden(hide_dpad).with_idle_timer(idle));
    let g = ui.clone();
    hook_at(sess, render, "-[GameView render:]", move |c| g.saw_game_view(c.reg(0)))?;
    let g = ui.clone();
    hook_at(sess, pause.0, "-[World pauseButtonTapped]", move |_| g.saw_pause())?;
    for (imp, cls, sel, dpad) in hidden {
        let g = ui.clone();
        let renders = sel.starts_with("render:");
        hook_or_skip(sess, imp, &format!("-[{cls} {sel}]"), move |_| {
            let off = if renders { g.off(dpad) } else { g.suppress(dpad) };
            off.then_some(0)
        })?;
    }
    sess.cpu.clear_cache();
    Ok(Arc::new(GameHooks { ui, pause, resume, chat, pinch }))
}

/// The "Game speed" option (see `gamespeed`): scale the steps `-[World update:accurateDT:pinchScale:dragInProgress:]`
/// gets while the world is not part of a multiplayer game.
fn game_speed(sess: &mut Session, speed: f64) -> Result<(), String> {
    let export = |name: &str| sess.linker.modules().iter().find_map(|m| m.export(name)).ok_or(format!("no {name}"));
    let class = export("OBJC_CLASS_$_World")?;
    let (client, server) = (export("OBJC_IVAR_$_World.client")?, export("OBJC_IVAR_$_World.server")?);
    let sel = "update:accurateDT:pinchScale:dragInProgress:";
    let imp = crate::fpsunlock::find_method(sess.cpu.mem(), class, sel, false).ok_or(format!("no -[World {sel}]"))?;
    hook_at(sess, imp, "-[World update:accurateDT:pinchScale:dragInProgress:]", move |c| {
        let ivar = |c: &Cpu, at: u32| {
            let off = c.mem().read_u32(at).unwrap_or(0);
            c.mem().read_u32(c.reg(0).wrapping_add(off)).unwrap_or(0)
        };
        let (cl, sv) = (ivar(c, client), ivar(c, server));
        for r in [2, 3] {
            let v = crate::gamespeed::scaled_step(c.reg(r), speed, cl, sv);
            c.set_reg(r, v);
        }
    })
}

/// Log the selector of every Objective-C method-cache miss (`lookUpMethod`), once per (class, selector).
fn hook_objc(sess: &mut Session) -> Result<(), String> {
    let seen = Mutex::new(std::collections::HashSet::new());
    hook_function(sess, "lookUpMethod", move |c| {
        let (cls, sel) = (c.reg(0), c.reg(1));
        let name = c.mem().read_cstr(sel).unwrap_or_default();
        if seen.lock().unwrap_or_else(PoisonError::into_inner).insert((cls, sel)) {
            eprintln!("[objc] t{} {cls:#x} {name}", sched::ctx().tid);
        }
    })
}

/// `--trace-fn NAME`: log every call of an exported ARM function with its first four arguments; an argument that
/// points at a `CFString` (or a C string) is shown as text.
fn hook_trace(sess: &mut Session, symbol: &str) -> Result<(), String> {
    let name = symbol.to_string();
    hook_function(sess, symbol, move |c| {
        let args: Vec<String> = (0..4)
            .map(|i| {
                let v = c.reg(i);
                let text = c
                    .mem()
                    .read_u32(v.wrapping_add(8))
                    .ok()
                    .and_then(|p| c.mem().read_cstr(p).ok())
                    .or_else(|| c.mem().read_cstr(v).ok())
                    .filter(|t| !t.is_empty() && t.chars().all(|ch| !ch.is_control()));
                match text {
                    Some(t) => format!("{v:#x}={t:?}"),
                    None => format!("{v:#x}"),
                }
            })
            .collect();
        eprintln!("[trace-fn] {name}({})", args.join(", "));
    })
}

/// The dev hooks of `BHPC_DEV_OBJC_HOOKS=1`, for the end-of-run report.
static DEV_HOOKS: std::sync::OnceLock<Arc<crate::objchook::ObjcHooks>> = std::sync::OnceLock::new();

fn report_objc_hooks(r: &crate::objchook::InstallReport) {
    for n in &r.installed {
        eprintln!("[dev-hook] installed {n}");
    }
    for (n, why) in &r.waiting {
        eprintln!("[dev-hook] waiting {n}: {why}");
    }
    for (n, why) in &r.failed {
        eprintln!("[dev-hook] failed {n}: {why}");
    }
}

/// Dev-only proof of the host ObjC hooks (`objchook`, the mod system's swizzling primitive): log the size and type
/// byte of every network message the client receives, time the HUD's renderer (an after hook) and look at the
/// world step's time (a before hook). Installed after the constructors on the main CPU.
fn dev_objc_hooks(sess: &mut Session) -> Option<(Arc<crate::objchook::ObjcHooks>, crate::objchook::Runtime)> {
    use crate::objchook::{ObjcHooks, Runtime, Target, Val};
    let rt = Runtime::resolve(|n| sess.linker.modules().iter().find_map(|m| m.export(n)))
        .inspect_err(|e| eprintln!("[dev-hook] no ObjC runtime: {e}"))
        .ok()?;
    let hooks = Arc::new(ObjcHooks::new());
    let t = |s: &str| Target::parse(s).expect("valid target");
    // -[BHClient match:didReceiveData:fromPlayer:]: [data length], first byte of [data bytes] (the packet type).
    let sels = Arc::new((AtomicU32::new(0), AtomicU32::new(0)));
    let packets = AtomicU64::new(0);
    hooks.hook(t("-[BHClient match:didReceiveData:fromPlayer:]"), None, move |inv| {
        let data = inv.arg(1).map_or(0, |v| v.word());
        let c = inv.cpu();
        let sel = |slot: &AtomicU32, name: &str, c: &mut Cpu| {
            let s = slot.load(Ordering::Relaxed);
            if s != 0 {
                return s;
            }
            let s = rt.sel(c, name).unwrap_or(0);
            slot.store(s, Ordering::Relaxed);
            s
        };
        let (len_sel, bytes_sel) = (sel(&sels.0, "length", c), sel(&sels.1, "bytes", c));
        let len = rt.send(c, data, len_sel, &[]).map_or(0, |r| r.r0);
        let ptr = rt.send(c, data, bytes_sel, &[]).map_or(0, |r| r.r0);
        let kind = c.mem().read_u8(ptr).unwrap_or(0);
        let n = packets.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 40 || n % 200 == 0 {
            eprintln!(
                "[dev-hook] -[BHClient match:didReceiveData:fromPlayer:] #{n} t{}: {len} bytes, type {kind:#04x}",
                sched::ctx().tid
            );
        }
    });
    // -[WorldUI render:translation:pinchScale:paused:]: after hook, timing the original (the first call, which
    // translates the whole HUD renderer into the nested JIT, is reported on its own).
    let renders = Mutex::new((0u64, Duration::ZERO));
    hooks.hook(t("-[WorldUI render:translation:pinchScale:paused:]"), None, move |inv| {
        let t0 = Instant::now();
        let r = inv.call_original();
        let took = t0.elapsed();
        let mut g = renders.lock().unwrap_or_else(PoisonError::into_inner);
        g.0 += 1;
        if g.0 > 1 {
            g.1 += took;
        }
        if g.0 == 1 || g.0 % 120 == 0 {
            let show = |v: Val| match v {
                Val::Words(w) => format!("{:?}", w.iter().map(|x| f32::from_bits(*x)).collect::<Vec<_>>()),
                v => format!("{v:?}"),
            };
            let args: Vec<String> = (0..4).filter_map(|i| inv.arg(i)).map(show).collect();
            let timing = if g.0 == 1 {
                format!("{:.0} us (first call)", took.as_secs_f64() * 1e6)
            } else {
                format!("{:.1} us avg after the first call", g.1.as_secs_f64() * 1e6 / (g.0 - 1) as f64)
            };
            eprintln!(
                "[dev-hook] -[WorldUI render:...] #{} t{}: original {timing}, args {}, result {r:?}",
                g.0,
                sched::ctx().tid,
                args.join(" ")
            );
        }
    });
    // -[World update:accurateDT:pinchScale:dragInProgress:]: before hook, the step's time.
    let steps = AtomicU64::new(0);
    hooks.hook(t("-[World update:accurateDT:pinchScale:dragInProgress:]"), None, move |inv| {
        let n = steps.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % 120 == 0 {
            let dt = inv.arg(0).unwrap_or(Val::Void);
            eprintln!("[dev-hook] -[World update:...] #{n}: dt {dt:?}");
        }
    });
    let t0 = Instant::now();
    report_objc_hooks(&hooks.install(&mut sess.cpu, &rt));
    eprintln!("[dev-hook] install took {:.0} us", t0.elapsed().as_secs_f64() * 1e6);
    let _ = DEV_HOOKS.set(hooks.clone());
    Some((hooks, rt))
}

/// `BHPC_DEV_OBJC_DUMP`: the method lists of the named classes (instance and class methods) with type encodings.
fn dev_objc_dump(sess: &Session, list: &std::path::Path) {
    let names = std::fs::read_to_string(list).unwrap_or_default();
    let mem = sess.cpu.mem();
    eprintln!("[objc-dump] {} class names from {}", names.lines().count(), list.display());
    for name in names.lines().map(str::trim).filter(|n| !n.is_empty()) {
        let Some(class) = sess.linker.modules().iter().find_map(|m| m.export(&format!("OBJC_CLASS_$_{name}"))) else {
            continue;
        };
        for meta in [false, true] {
            let read = || -> Option<Vec<String>> {
                let cls = if meta { mem.read_u32(class).ok()? } else { class };
                let mut ro = mem.read_u32(cls + 16).ok()? & !3;
                // A realised class points at its class_rw_t (flags bit 31 RW_REALIZED; ro at +8).
                if mem.read_u32(ro).ok()? & (1 << 31) != 0 {
                    ro = mem.read_u32(ro + 8).ok()?;
                }
                let list = mem.read_u32(ro + 20).ok()?;
                if list == 0 {
                    return Some(Vec::new());
                }
                let size = mem.read_u32(list).ok()? & !3;
                let count = mem.read_u32(list + 4).ok()?;
                let mut out = Vec::new();
                for i in 0..count.min(4096) {
                    let m = list + 8 + i * size;
                    let sel = mem.read_cstr(mem.read_u32(m).ok()?).ok()?;
                    let ty = mem.read_cstr(mem.read_u32(m + 4).ok()?).ok()?;
                    let imp = mem.read_u32(m + 8).ok()?;
                    out.push(format!("{}[{name} {sel}] {ty} imp={imp:#x}", if meta { '+' } else { '-' }));
                }
                Some(out)
            };
            let got = read();
            if got.is_none() {
                eprintln!("[objc-dump] {name}: cannot read the method list");
            }
            for l in got.unwrap_or_default() {
                eprintln!("[objc-dump] {l}");
            }
        }
    }
}
