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
use crate::embed::{cmd_event, rgba_bottom_up_to_bgra, InputTracker};
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
use bhpc_launcher::embed::{Cmd, FrameWriter};
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
    /// Button index to press when the game shows an alert (default: the first button, else the cancel button).
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
    /// Window title.
    pub title: String,
    /// What the buttons, wheel and keys do.
    pub bindings: Bindings,
    /// Render for the launcher's window instead of one of our own: the frame file the launcher created. The video
    /// driver is forced to SDL's offscreen one, input arrives as `bhpc_launcher::embed::Cmd` lines on stdin and the
    /// run ends when stdin closes.
    pub embed: Option<PathBuf>,
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
            alert_choice: None,
            script: Vec::new(),
            shot_dir: PathBuf::from("."),
            fullscreen: false,
            vsync: true,
            fps_limit: None,
            show_fps: false,
            title: "The Blockheads".into(),
            bindings: Bindings::default(),
            embed: None,
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
    }
}

#[derive(Default)]
struct Flags {
    done: AtomicBool,
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
    /// In-window mode: where finished frames go.
    embed: Option<Mutex<EmbedOut>>,
    /// The SDL window, for the read-back that does not depend on the window's own surface (in-window mode).
    sdl: Arc<bhpc_app::sdl::SdlGl>,
    /// The frame-rate cap and the FPS counter.
    pace: Mutex<crate::pace::SwapPacer>,
}

/// Frame file and scratch buffer of the in-window mode.
struct EmbedOut {
    writer: FrameWriter,
    last: Option<Instant>,
    bgra: Vec<u8>,
    /// Read-back buffer, reused.
    rgba: Vec<u8>,
    /// Frames published and time spent reading back, converting and writing them since the last report.
    stats: (u64, Duration, Instant),
}

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
        let EmbedOut { writer, last, bgra, rgba, stats } = &mut *e;
        let Some((w, h)) = self.sdl.compose_read(rgba) else { return };
        rgba_bottom_up_to_bgra(w, h, rgba, bgra);
        match writer.publish(w, h, bgra) {
            Ok(true) => {
                *last = Some(Instant::now());
                stats.0 += 1;
                stats.1 += began.elapsed();
                if stats.2.elapsed() >= Duration::from_secs(10) {
                    let ms = stats.1.as_secs_f64() * 1000.0 / stats.0.max(1) as f64;
                    eprintln!(
                        "[embed] frame path: shared frame file, {w}x{h}, {:.1} fps, {ms:.2} ms per frame spent in read-back, convert and write",
                        stats.0 as f64 / stats.2.elapsed().as_secs_f64()
                    );
                    *stats = (0, Duration::ZERO, Instant::now());
                }
            }
            Ok(false) => eprintln!("[play] a {w}x{h} frame does not fit the frame file"),
            Err(e) => eprintln!("[play] cannot write the frame file: {e}"),
        }
    }

    /// Count a presented frame (and take the screenshot when it is due); call right before the swap.
    fn before_swap(&self) {
        // The frame is drawn: wait for its slot under the frame-rate cap (other guest threads keep running), then
        // present it. The game measures real time between frames, so a capped game just sees longer frames.
        let height = self.window.surface_size().1.min(self.window.drawable_size().1);
        if let Some(hud) = self.pace.lock().unwrap_or_else(PoisonError::into_inner).frame(height) {
            self.sdl.set_hud(Some(hud));
        }
        // The Java views (popups) go over the game's frame, also in screenshots.
        let vm = self.flags.vm.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(vm) = vm {
            self.overlay.update(&*self.window, vm.render_ui());
        }
        self.publish_frame();
        let n = self.flags.swaps.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            emit(Line::Ready);
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
        // No window of our own: SDL's offscreen driver gives the GL context, the launcher shows the frames.
        std::env::set_var("SDL_VIDEODRIVER", "offscreen");
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
    let (mut backend, gl) =
        SdlBackend::new(&WindowConfig { title: cfg.title.clone(), size: cfg.window, ..WindowConfig::default() })
            .map_err(|e| no_window(cfg, e))?;
    gl.set_vsync_allowed(cfg.vsync);
    let mut icon = include_bytes!("../assets/icon-32.rgba").to_vec();
    backend.set_icon(&mut icon, 32, 32);
    let m0 = backend.metrics();
    eprintln!("[play] video driver {}, window {:?}, drawable {:?}", backend.driver(), m0.logical, m0.drawable);
    // What the window system really gave us is what the game sees (a compositor may ignore the requested size).
    let mut cfg = cfg.clone();
    (cfg.window, cfg.drawable) = (m0.logical, m0.drawable);
    let cfg = &cfg;
    let mixer = Mixer::new(44100);
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
    let embed_cfg = cfg.clone();
    let embed_out: Mutex<Option<EmbedOut>> = Mutex::new(match &cfg.embed {
        Some(path) => {
            let writer = FrameWriter::open(path).map_err(|e| other(format!("frame file {}: {e}", path.display())))?;
            Some(EmbedOut {
                writer,
                last: None,
                bgra: Vec::new(),
                rgba: Vec::new(),
                stats: (0, Duration::ZERO, Instant::now()),
            })
        }
        None => None,
    });
    let (shot, shot_at, check_gl) = (cfg.shot.clone(), cfg.shot_at, cfg.check_gl);
    let (fps_limit, show_fps) = (cfg.fps_limit, cfg.show_fps);
    let mut sess = Session::open_setup(&icfg, Box::new(std::io::stdout()), None, &mut |cpu, st| {
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
            sdl: sdl.clone(),
            pace: Mutex::new(crate::pace::SwapPacer::new(lock(st).sched.clone(), fps_limit, show_fps)),
            embed: embed_out.lock().unwrap_or_else(PoisonError::into_inner).take().map(Mutex::new),
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
        Ok(HostHooks { gfx: Some(gfx), audio: Some(audio) })
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
    for f in &cfg.trace_fn {
        if let Err(e) = hook_trace(&mut sess, f) {
            eprintln!("[play] cannot trace {f}: {e}");
        }
    }
    let mut report = PlayReport::default();
    let vm = sess.jni.vm.clone();
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
        gles.set_frame_hook(Arc::new(move |c| q.drain(c)));
    }
    let ui = UiThread {
        main: Some(main_q),
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
        let rx = self.rx.lock().unwrap_or_else(PoisonError::into_inner).take();
        let mut mapper = InputMapper::new(InputConfig {
            window: self.cfg.window,
            bindings: self.cfg.bindings,
            ..InputConfig::default()
        });
        Ok(self.ui_loop(c, &mut mapper, rx.as_ref()))
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
        if let Err(Throw::Stop(s)) = self.vm.alert_click(c, alert, pick) {
            let msg = format!("alert: guest stopped: {s}");
            *self.flags.fail.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((msg, c.stop_regs().unwrap_or([0; 16])));
        }
    }

    fn ui_loop(&self, c: &mut Cpu, mapper: &mut InputMapper, rx: Option<&Receiver<Event>>) -> String {
        let (vm, flags, cfg, st) = (&self.vm, &self.flags, &self.cfg, &self.st);
        let begun = Instant::now();
        let mut tracker = InputTracker::default();
        let mut text_alert: Option<bhpc_jni::Alert> = None;
        let mut focused: Option<bhpc_jni::Obj> = None;
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
            sched::sleep(c, st, 4_000_000);
            // Touch times are `SystemClock.uptimeMillis`; UIKit compares them with `systemUptime`, the guest's
            // monotonic clock, to expire taps.
            let now = {
                let (secs, nanos) = lock(st).clock.monotonic();
                secs * 1000 + i64::from(nanos / 1_000_000)
            };
            // Runnables other threads posted to the Android main looper (`performSelectorOnMainThread` and friends).
            c.set_ticks(FOREVER);
            match vm.pump(c, vm.main_looper(), vm.now_ms()) {
                Ok(_) => {}
                Err(Throw::Stop(s)) => return format!("main looper: guest stopped: {s}"),
                Err(Throw::Exc(e)) => {
                    eprintln!("[play] main looper: Java exception {}", vm.exception_message(e).unwrap_or_default());
                }
            }
            if let Some((run, mode)) = self.runloop {
                // `CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, true)`: the double sits in r2:r3, the flag on the stack.
                c.set_ticks(FOREVER);
                if let Err(e) = c.call(run, &[mode, 0, 0, 0, 1]) {
                    return format!("UI run loop: guest stopped: {e:?}");
                }
            }
            match vm.run_layout(c) {
                Ok(_) => {}
                Err(Throw::Stop(s)) => return format!("layout: guest stopped: {s}"),
                Err(Throw::Exc(e)) => {
                    eprintln!("[play] layout: Java exception {}", vm.exception_message(e).unwrap_or_default());
                }
            }
            for alert in vm.take_alerts() {
                if alert.field.is_some() {
                    // An alert with an input line waits for the typed text; Return answers it with the first button,
                    // Escape with the cancel button.
                    eprintln!("[play] alert {:?} with a text field: waiting for text input", alert.title);
                    text_alert = Some(alert);
                    continue;
                }
                self.answer(c, &alert, None);
                if flags.fail.lock().unwrap_or_else(PoisonError::into_inner).is_some() {
                    return "alert: guest stopped".into();
                }
            }
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
            };
            let mut escape = false;
            // The game announced (or changed) its content size: touches map into it.
            if let Some((w, h)) = vm.content_size().filter(|s| Some(*s) != mapper.geometry().content) {
                mapper.set_content(w, h);
            }
            let pending: Vec<Event> = std::iter::from_fn(|| rx.and_then(|r| r.try_recv().ok())).collect();
            for ev in crate::input::coalesce_moves(pending) {
                if let Event::Key { key, pressed: true, mods, .. } = &ev {
                    let held = bhpc_input::Mods { ctrl: mods.ctrl, shift: mods.shift, alt: mods.alt };
                    escape |= cfg.bindings.back.fires(Input::Key(host_key(*key)), held);
                }
                if let Event::Resized(m) = &ev {
                    // The display of the game follows the window; `SurfaceChanged` below tells the game.
                    vm.set_screen_size(m.drawable.0 as i32, m.drawable.1 as i32);
                    eprintln!("[play] window {:?}, drawable {:?}", m.logical, m.drawable);
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
            if let Some(alert) = text_alert.take_if(|_| returned || escape) {
                let pick = if returned { None } else { Some(alert.cancel.as_ref().map_or(0, |b| b.1)) };
                eprintln!("[play] text alert answered with {:?}: {:?}", pick, alert.field.map(|f| vm.text_of(f)));
                self.answer(c, &alert, pick);
                let mut sink = JniSink {
                    vm,
                    cpu: c,
                    actions: Vec::new(),
                    errors: Vec::new(),
                    returned: false,
                    main: self.main.clone(),
                };
                mapper.set_text_focus(None, &mut sink);
                focused = None;
            }
        }
    }
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
    let entry = sess.linker.modules().iter().find_map(|m| m.export(symbol)).ok_or(format!("no {symbol} export"))?;
    if entry & 1 != 0 {
        return Err(format!("{symbol} is Thumb code"));
    }
    let mem = sess.cpu.mem().clone();
    let tramp = lock(&sess.st).heap.alloc(16);
    let orig = mem.read_bytes(entry, 8).map_err(|e| e.to_string())?;
    let mut code = orig.clone();
    code.extend_from_slice(&0xE51F_F004u32.to_le_bytes()); // ldr pc, [pc, #-4]
    code.extend_from_slice(&(entry + 8).to_le_bytes());
    mem.write_bytes(tramp, &code).map_err(|e| e.to_string())?;
    let thunk = sess
        .cpu
        .register_shared_thunk(
            &format!("bhpc:trace:{symbol}"),
            Arc::new(move |c: &mut Cpu| {
                hook(c);
                c.tail_call(tramp);
            }),
        )
        .map_err(|e| e.to_string())?;
    let mut patch = 0xE51F_F004u32.to_le_bytes().to_vec();
    patch.extend_from_slice(&thunk.to_le_bytes());
    mem.write_bytes(entry, &patch).map_err(|e| e.to_string())?;
    sess.cpu.clear_cache();
    Ok(())
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
