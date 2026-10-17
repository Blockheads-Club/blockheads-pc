//! The main loop: pumps events, handles fullscreen toggles and resizes, calls the render hook, presents and paces.

use crate::event::{Event, Key, Mods};
use crate::metrics::Metrics;
use crate::pacer::{Clock, FramePacer};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// The windowing system underneath [`App`]. Implemented by the SDL2 backend and by fakes in tests.
pub trait Backend {
    /// Next event, waiting at most `timeout` (zero = do not wait).
    fn poll_event(&mut self, timeout: Duration) -> Option<Event>;
    fn metrics(&self) -> Metrics;
    /// Switch between (borderless desktop) fullscreen and windowed mode. False if the switch failed.
    fn set_fullscreen(&mut self, on: bool) -> bool;
    /// Present the frame the render hook drew (buffer swap).
    fn present(&mut self);
    /// Enable or disable vertical sync. False if unsupported.
    fn set_vsync(&mut self, on: bool) -> bool;
    fn set_title(&mut self, title: &str);
}

/// What the loop does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// One frame handed to the render hook.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Frame counter, starting at 0.
    pub number: u64,
    /// Time since the previous frame (zero for the first one).
    pub dt: Duration,
    pub metrics: Metrics,
    pub fps: f32,
}

/// The application side: input and rendering hooks.
pub trait Handler {
    /// An event the loop did not consume itself.
    fn event(&mut self, event: &Event) -> Flow;
    /// Draw one frame (only called when [`AppOptions::render_in_loop`] is set).
    fn render_frame(&mut self, frame: &Frame) -> Flow;
}

#[derive(Debug, Clone)]
pub struct AppOptions {
    /// Handle F11 and Alt+Enter as fullscreen toggles (the keys are not forwarded).
    pub fullscreen_keys: bool,
    /// Start in fullscreen.
    pub fullscreen: bool,
    /// Ask the backend for vsync. If it refuses, `fallback_fps` paces the loop.
    pub vsync: bool,
    /// Frame cap used without vsync (`None` = uncapped).
    pub fallback_fps: Option<u32>,
    /// Call [`Handler::render_frame`] and present on this thread. Off when the guest's GL thread renders: the loop
    /// then only pumps events.
    pub render_in_loop: bool,
    /// How long the event pump blocks when it does not render.
    pub idle_wait: Duration,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            fullscreen_keys: true,
            fullscreen: false,
            vsync: true,
            fallback_fps: Some(60),
            render_in_loop: true,
            idle_wait: Duration::from_millis(4),
        }
    }
}

/// Counters returned by [`App::run`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunStats {
    pub frames: u64,
    pub events: u64,
}

pub struct App<B: Backend, C: Clock> {
    backend: B,
    clock: C,
    opts: AppOptions,
    pacer: FramePacer,
    fullscreen: bool,
    vsync: bool,
}

impl<B: Backend, C: Clock> App<B, C> {
    pub fn new(mut backend: B, clock: C, opts: AppOptions) -> App<B, C> {
        let vsync = opts.vsync && backend.set_vsync(true);
        if !vsync {
            backend.set_vsync(false);
        }
        let fullscreen = opts.fullscreen && backend.set_fullscreen(true);
        let pacer = FramePacer::new(if vsync { None } else { opts.fallback_fps });
        App { backend, clock, opts, pacer, fullscreen, vsync }
    }

    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// True if vsync was granted (and the loop therefore does not sleep).
    pub fn vsync(&self) -> bool {
        self.vsync
    }

    pub fn fps(&self) -> f32 {
        self.pacer.fps()
    }

    pub fn set_title(&mut self, title: &str) {
        self.backend.set_title(title);
    }

    /// Toggle fullscreen; the handler is told through [`Event::Fullscreen`] by the caller of this method.
    pub fn toggle_fullscreen(&mut self) -> bool {
        let want = !self.fullscreen;
        if self.backend.set_fullscreen(want) {
            self.fullscreen = want;
        }
        self.fullscreen
    }

    fn is_toggle(&self, key: Key, pressed: bool, repeat: bool, mods: Mods) -> bool {
        self.opts.fullscreen_keys && pressed && !repeat && (key == Key::F11 || (key == Key::ENTER && mods.alt))
    }

    /// Handle one event; `Quit` ends the loop.
    fn dispatch(&mut self, ev: Event, h: &mut dyn Handler) -> Flow {
        match &ev {
            Event::Key { key, pressed, repeat, mods } if self.is_toggle(*key, *pressed, *repeat, *mods) => {
                let on = self.toggle_fullscreen();
                return h.event(&Event::Fullscreen(on));
            }
            Event::Quit => {
                h.event(&ev);
                return Flow::Quit;
            }
            _ => {}
        }
        h.event(&ev)
    }

    /// Run until the window is closed or a hook returns [`Flow::Quit`].
    pub fn run(&mut self, h: &mut dyn Handler) -> RunStats {
        let mut stats = RunStats::default();
        let mut last_dt = Duration::ZERO;
        loop {
            let mut timeout = if self.opts.render_in_loop { Duration::ZERO } else { self.opts.idle_wait };
            while let Some(ev) = self.backend.poll_event(timeout) {
                timeout = Duration::ZERO;
                stats.events += 1;
                if self.dispatch(ev, h) == Flow::Quit {
                    return stats;
                }
            }
            if !self.opts.render_in_loop {
                continue;
            }
            let frame =
                Frame { number: stats.frames, dt: last_dt, metrics: self.backend.metrics(), fps: self.pacer.fps() };
            let flow = h.render_frame(&frame);
            self.backend.present();
            last_dt = self.pacer.end_frame(&mut self.clock).dt;
            stats.frames += 1;
            if flow == Flow::Quit {
                return stats;
            }
        }
    }
}

/// A [`Handler`] that forwards events to another thread (the guest input thread) and never renders.
pub struct ForwardHandler {
    tx: Sender<Event>,
}

impl ForwardHandler {
    pub fn new(tx: Sender<Event>) -> ForwardHandler {
        ForwardHandler { tx }
    }
}

impl Handler for ForwardHandler {
    fn event(&mut self, event: &Event) -> Flow {
        if self.tx.send(event.clone()).is_err() {
            return Flow::Quit;
        }
        Flow::Continue
    }

    fn render_frame(&mut self, _frame: &Frame) -> Flow {
        Flow::Continue
    }
}
