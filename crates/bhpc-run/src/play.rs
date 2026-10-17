//! `bhpc-run --apk <apk>` without `--init-only`: open an SDL window with a GLES2 context, wire GL, OpenSL ES,
//! `dlopen` and the fake JVM into the loader, run the Java-side start-up sequence and pump input.
//!
//! Threads: the host main thread owns the window and its event pump (`bhpc-app`); the "guest main" host thread
//! owns the [`Session`] and acts as the Android UI thread (start-up stages, input delivery); `MainThread.nativeRun`
//! runs on a real guest thread (a `pthread_create`d thunk with a big stack).
//!
//! Not part of the coverage gate (needs a display); the pieces with logic live in `png`, `input` and `dl`.

use crate::apkrun::{backtrace_lines, InitConfig, Session};
use crate::dl::{self, DlTable};
use crate::input::{host_event, JniSink};
use crate::{png, RunError};
use bhpc_app::sdl::SdlBackend;
use bhpc_app::{App, AppOptions, Backend, Event, ForwardHandler, Metrics, SystemClock, WindowConfig};
use bhpc_audio::{Mixer, Sles};
use bhpc_bionic::{lock, sched};
use bhpc_cpu::Cpu;
use bhpc_gles::{GlWindow, Gles, GlesConfig};
use bhpc_input::{Config as InputConfig, HostAction, InputMapper};
use bhpc_jni::{GfxHooks, Stage, Throw, Vm};
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
    /// Write the full list of Java members the guest looked up (`host`, `native`, `STUB`, call counts) here.
    pub jni_report: Option<PathBuf>,
}

impl PlayConfig {
    pub fn new(init: InitConfig) -> PlayConfig {
        PlayConfig {
            init,
            window: (1024, 768),
            shot: None,
            shot_at: 120,
            exit_after_shot: false,
            seconds: None,
            check_gl: false,
            symbolize: Vec::new(),
            jni_report: None,
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
    /// Registers of the faulting guest thread.
    fail: Mutex<Option<(String, [u32; 16])>>,
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
}

type ReadPixels = unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void);
type GetIntegerv = unsafe extern "C" fn(u32, *mut i32);

impl Hooks {
    fn egl(&self, java_id: u32) -> Option<u32> {
        self.ctxs.lock().unwrap_or_else(PoisonError::into_inner).get(&java_id).copied()
    }

    /// Read the default framebuffer back (RGBA, top row first). The context must be current on this thread.
    fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
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
            let mut fbo = 0;
            gi(0x8CA6, &mut fbo);
            if fbo != 0 {
                return None;
            }
            let mut buf = vec![0u8; w as usize * h as usize * 4];
            rp(0, 0, w as i32, h as i32, 0x1908, 0x1401, buf.as_mut_ptr().cast());
            Some((w, h, png::flip_rows(w, h, &buf)))
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
        let n = self.flags.swaps.fetch_add(1, Ordering::SeqCst) + 1;
        self.snapshot(n);
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
    }

    fn context_lost(&self) {
        eprintln!("[play] game reported a lost EGL context");
    }
}

/// Backend wrapper that turns the shared "done" flag into a `Quit` event, so the event loop ends when the guest does.
struct Watched<B: Backend> {
    inner: B,
    flags: Arc<Flags>,
}

impl<B: Backend> Backend for Watched<B> {
    fn poll_event(&mut self, timeout: Duration) -> Option<Event> {
        if self.flags.done.load(Ordering::SeqCst) {
            return Some(Event::Quit);
        }
        self.inner.poll_event(timeout)
    }
    fn metrics(&self) -> Metrics {
        self.inner.metrics()
    }
    fn set_fullscreen(&mut self, on: bool) -> bool {
        self.inner.set_fullscreen(on)
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
    let (backend, gl) =
        SdlBackend::new(&WindowConfig { size: cfg.window, ..WindowConfig::default() }).map_err(other)?;
    eprintln!("[play] video driver {}", backend.driver());
    let flags = Arc::new(Flags::default());
    let (tx, rx) = channel();
    let (c, f) = (cfg.clone(), flags.clone());
    let guest = std::thread::Builder::new()
        .name("guest-main".into())
        .stack_size(256 << 20)
        .spawn(move || guest_main(&c, gl, rx, f))?;
    let mut app = App::new(
        Watched { inner: backend, flags: flags.clone() },
        SystemClock::new(),
        AppOptions { render_in_loop: false, vsync: false, fallback_fps: None, ..AppOptions::default() },
    );
    app.run(&mut ForwardHandler::new(tx));
    flags.done.store(true, Ordering::SeqCst);
    let report = guest.join().map_err(|_| other("the guest main thread panicked"))?;
    drop(app);
    report
}

/// Pieces created while the session is being opened.
struct Parts {
    gles: Gles,
    sles: Arc<Sles>,
    primary: u32,
    surface: u32,
}

fn guest_main(
    cfg: &PlayConfig,
    gl: Arc<bhpc_app::sdl::SdlGl>,
    rx: Receiver<Event>,
    flags: Arc<Flags>,
) -> Result<PlayReport, RunError> {
    let result = boot(cfg, gl, rx, &flags);
    flags.done.store(true, Ordering::SeqCst);
    result
}

fn boot(
    cfg: &PlayConfig,
    gl: Arc<bhpc_app::sdl::SdlGl>,
    rx: Receiver<Event>,
    flags: &Arc<Flags>,
) -> Result<PlayReport, RunError> {
    let window: Arc<dyn GlWindow> = gl;
    let mixer = Mixer::new(44100);
    let dl_slot: Arc<Mutex<Option<DlTable>>> = Arc::new(Mutex::new(None));
    let mut parts: Option<Parts> = None;
    let blocks: BlockLog = Arc::default();
    let mut icfg = cfg.init.clone();
    icfg.watchdog_cycles = FOREVER;
    icfg.screen = Some((cfg.window.0 as i32, cfg.window.1 as i32));
    let hooks_flags = flags.clone();
    let (shot, shot_at, check_gl) = (cfg.shot.clone(), cfg.shot_at, cfg.check_gl);
    let mut sess = Session::open_setup(&icfg, Box::new(std::io::stdout()), None, &mut |cpu, st| {
        let primary = window.create_context(None).ok_or_else(|| other("cannot create a GLES2 context"))?;
        if !window.make_current(Some(primary)) {
            return Err(other("cannot make the GL context current"));
        }
        let mut gcfg = GlesConfig::new(GLES_ARENA, 8 << 20);
        gcfg.check_errors = check_gl;
        let gles = Gles::with_host(gcfg, window.clone(), cpu.mem().clone()).map_err(RunError::Mem)?;
        gles.egl_initialize();
        let pid = gles.egl_adopt_context(primary);
        let surface = gles.egl_create_surface();
        bhpc_gles::register_all(cpu, &gles);
        window.make_current(None);
        let sles = Sles::install(cpu, mixer.clone()).map_err(other)?;
        dl::install(cpu, st, dl_slot.clone());
        watch_blocking(cpu, st, &blocks);
        let mut ctxs = HashMap::new();
        ctxs.insert(1, pid);
        let hooks = Hooks {
            gles: gles.clone(),
            window: window.clone(),
            surface,
            primary: pid,
            ctxs: Mutex::new(ctxs),
            next_id: AtomicU32::new(2),
            flags: hooks_flags.clone(),
            shot: shot.clone(),
            shot_at,
        };
        parts = Some(Parts { gles, sles, primary: pid, surface });
        let hooks: Arc<dyn GfxHooks> = Arc::new(hooks);
        Ok(Some(hooks))
    })?;
    let Parts { gles, sles, primary, surface } = parts.ok_or_else(|| other("setup did not run"))?;
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
    let mut report = PlayReport::default();
    let vm = sess.jni.vm.clone();
    let pid = sess.cpu.processor_id();

    // 1. Constructors, JNI_OnLoad and libv_init of every library.
    let (_, failures) = sess.run_constructors(&icfg)?;
    if let Some(f) = failures.first() {
        report.stop = format!("constructor failed: {} ({})", f.failure, f.stop);
        report.backtrace = f.backtrace.clone();
        return Ok(finish(sess, &gles, report, flags, cfg, &blocks));
    }
    eprintln!("[play] constructors done");

    // 2. Java start-up on this thread (the UI thread).
    let run = |sess: &mut Session, stage: Stage| -> Result<(), String> {
        sess.cpu.set_ticks(FOREVER);
        eprintln!("[play] stage {stage:?} (bound: {})", vm.stage_bound(stage));
        match vm.run_stage(&mut sess.cpu, stage) {
            Ok(()) => Ok(()),
            Err(Throw::Stop(s)) => {
                let regs = sess.cpu.stop_regs();
                let bt = regs.map(|r| backtrace_lines(&sess.linker, &sess.cpu, r)).unwrap_or_default();
                Err(format!("{stage:?}: guest stopped: {s}\n  {}", bt.join("\n  ")))
            }
            Err(Throw::Exc(e)) => {
                Err(format!("{stage:?}: Java exception {}", vm.exception_message(e).unwrap_or_default()))
            }
        }
    };
    let start = |sess: &mut Session| -> Result<(), String> {
        run(sess, Stage::LibraryInit)?;
        let env = bhpc_jni::native_env(&sess.platform);
        sess.cpu.set_ticks(FOREVER);
        vm.set_native_env(&mut sess.cpu, &env).map_err(|e| format!("setenv: {e:?}"))?;
        vm.attach_main_looper(pid);
        for s in [Stage::ActivityCreate, Stage::ActivityStart, Stage::ActivityResume, Stage::RootGeometry] {
            run(sess, s)?;
        }
        // The GL context becomes valid on this thread; the game's own thread takes it over afterwards.
        gles.egl_make_current(surface, surface, primary).map_err(|e| format!("eglMakeCurrent failed: {e:#x}"))?;
        for s in [
            Stage::SurfaceInit,
            Stage::ContextValid,
            Stage::SurfaceChanged,
            Stage::WillEnterForeground,
            Stage::DidBecomeActive,
        ] {
            run(sess, s)?;
        }
        let _ = gles.egl_make_current(0, 0, 0);
        Ok(())
    };
    if let Err(e) = start(&mut sess) {
        report.stop = e;
        return Ok(finish(sess, &gles, report, flags, cfg, &blocks));
    }

    // 3. Audio callbacks, then the game's main thread.
    let stack = sess.cpu.mem().map_anywhere(256 << 10, 4096)?;
    let _pump = sles.spawn_pump(&sess.cpu, stack + (256 << 10))?;
    if let Err(e) = spawn_main_thread(&mut sess, &vm, flags) {
        report.stop = e;
        return Ok(finish(sess, &gles, report, flags, cfg, &blocks));
    }

    // 4. This thread is now the UI thread: hand the baton over and deliver input.
    let begun = Instant::now();
    let mut mapper = InputMapper::new(InputConfig { window: cfg.window, ..InputConfig::default() });
    report.stop = ui_loop(cfg, &mut sess, &vm, &mut mapper, &rx, flags, begun);
    Ok(finish(sess, &gles, report, flags, cfg, &blocks))
}

/// Create the guest thread that runs `MainThread.nativeRun` (a thunk as the start routine).
fn spawn_main_thread(sess: &mut Session, vm: &Vm, flags: &Arc<Flags>) -> Result<(), String> {
    let (vm, flags) = (vm.clone(), flags.clone());
    let entry = sess
        .cpu
        .register_shared_thunk(
            "bhpc:main_thread",
            Arc::new(move |c: &mut Cpu| {
                c.set_ticks(FOREVER);
                eprintln!("[play] MainThread.nativeRun starts");
                match vm.run_stage(c, Stage::MainThreadRun) {
                    Ok(()) => eprintln!("[play] MainThread.nativeRun returned"),
                    Err(Throw::Stop(s)) => {
                        let regs = c.stop_regs().unwrap_or([0; 16]);
                        *flags.fail.lock().unwrap_or_else(PoisonError::into_inner) =
                            Some((format!("MainThread.nativeRun: guest stopped: {s}"), regs));
                    }
                    Err(Throw::Exc(e)) => {
                        let msg = format!(
                            "MainThread.nativeRun: Java exception {}",
                            vm.exception_message(e).unwrap_or_default()
                        );
                        *flags.fail.lock().unwrap_or_else(PoisonError::into_inner) = Some((msg, [0; 16]));
                    }
                }
                c.ret_u32(0);
            }),
        )
        .map_err(|e| e.to_string())?;
    let create = bhpc_bionic::resolve(&mut sess.cpu, &sess.st, "pthread_create").ok_or("no pthread_create")?;
    let area = lock(&sess.st).heap.calloc(1, 64);
    if area == 0 {
        return Err("guest heap exhausted".into());
    }
    sess.cpu.mem().write_u32(area + 8 + 8, MAIN_STACK).map_err(|e| e.to_string())?;
    sess.cpu.set_ticks(FOREVER);
    match sess.cpu.call(create, &[area, area + 8, entry, 0]) {
        Ok(r) if r.r0 == 0 => Ok(()),
        Ok(r) => Err(format!("pthread_create failed: {}", r.r0)),
        Err(s) => Err(format!("pthread_create stopped: {s}")),
    }
}

fn ui_loop(
    cfg: &PlayConfig,
    sess: &mut Session,
    vm: &Vm,
    mapper: &mut InputMapper,
    rx: &Receiver<Event>,
    flags: &Arc<Flags>,
    begun: Instant,
) -> String {
    let st = sess.st.clone();
    loop {
        if flags.done.load(Ordering::SeqCst) {
            return "window closed".into();
        }
        if cfg.exit_after_shot && flags.shot_taken.load(Ordering::SeqCst) {
            return "screenshot taken".into();
        }
        if cfg.seconds.is_some_and(|s| begun.elapsed() >= Duration::from_secs(s)) {
            return format!("time limit reached after {} swaps", flags.swaps.load(Ordering::SeqCst));
        }
        if let Some((msg, _)) = flags.fail.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return msg;
        }
        if let Some(f) = lock(&st).sched.failure() {
            return format!("a guest thread failed: {f}");
        }
        sched::sleep(&mut sess.cpu, &st, 4_000_000);
        let now = begun.elapsed().as_millis() as i64;
        // Runnables other threads posted to the Android main looper (`performSelectorOnMainThread` and friends).
        sess.cpu.set_ticks(FOREVER);
        match vm.pump(&mut sess.cpu, vm.main_looper(), vm.now_ms()) {
            Ok(_) => {}
            Err(Throw::Stop(s)) => return format!("main looper: guest stopped: {s}"),
            Err(Throw::Exc(e)) => {
                eprintln!("[play] main looper: Java exception {}", vm.exception_message(e).unwrap_or_default())
            }
        }
        match vm.run_layout(&mut sess.cpu) {
            Ok(_) => {}
            Err(Throw::Stop(s)) => return format!("layout: guest stopped: {s}"),
            Err(Throw::Exc(e)) => {
                eprintln!("[play] layout: Java exception {}", vm.exception_message(e).unwrap_or_default());
            }
        }
        let mut sink = JniSink { vm, cpu: &mut sess.cpu, actions: Vec::new(), errors: Vec::new() };
        while let Ok(ev) = rx.try_recv() {
            if let Some(h) = host_event(&ev) {
                mapper.handle(h, now, &mut sink);
            }
        }
        mapper.tick(now, &mut sink);
        for e in sink.errors.drain(..) {
            eprintln!("[play] input delivery: {e}");
        }
        if sink.actions.contains(&HostAction::Quit) {
            return "quit requested".into();
        }
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
        for l in backtrace_lines(&sess.linker, &sess.cpu, *regs).iter().take(12) {
            eprintln!("    {l}");
        }
    }
    for a in sym {
        eprintln!("[sym] {}", crate::apkrun::symbolize(&sess.linker, &sess.cpu, *a));
    }
    r.swaps = flags.swaps.load(Ordering::SeqCst);
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
    r.top_thunks = bhpc_bionic::histogram(&sess.st).into_iter().filter(|(_, c)| *c > 0).take(20).collect();
    sess.shutdown();
    r
}
