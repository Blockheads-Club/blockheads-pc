//! Window and main-loop skeleton for the PC port (PLAN section 1.5).
//!
//! * [`App`] pumps events from a [`Backend`], consumes the fullscreen keys, tracks [`Metrics`] (size and DPI
//!   scale), calls the application's [`Handler`] for events and (optionally) for each frame, presents and paces.
//! * Two modes: `render_in_loop` (this thread renders, used by tests and the first-frame milestone) or event
//!   pump only (the guest's GLThread renders through `bhpc-gles` and the loop forwards input with
//!   [`ForwardHandler`]).
//! * [`hud`] renders the optional FPS counter that the window blends over the game picture.
//! * The SDL2 backend (`sdl` feature) creates a GLES 2.0 window and implements `bhpc_gles::GlWindow`.
//!
//! Everything except the SDL glue is platform independent and tested headlessly with a fake backend.

pub mod app;
pub mod event;
pub mod hud;
pub mod metrics;
pub mod pacer;
#[cfg(feature = "sdl")]
pub mod sdl;
pub mod surface;
#[cfg(feature = "sdl")]
mod vsurface;

#[cfg(test)]
mod tests_app;

pub use app::{App, AppOptions, Backend, Flow, ForwardHandler, Frame, Handler, RunStats};
pub use event::{Event, Key, Mods, MouseButton, TouchPhase};
pub use metrics::Metrics;
pub use pacer::{Clock, FpsCounter, FramePacer, FrameTiming, SystemClock};

/// Window creation parameters.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    /// Initial size in logical pixels.
    pub size: (u32, u32),
    pub resizable: bool,
    /// Request a high-DPI drawable.
    pub high_dpi: bool,
    /// Open game controllers and report their input.
    pub controllers: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig {
            title: "The Blockheads".into(),
            size: (1280, 720),
            resizable: true,
            high_dpi: true,
            controllers: true,
        }
    }
}
