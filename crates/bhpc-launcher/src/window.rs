//! Window and graphics options.

use serde::{Deserialize, Serialize};

/// Smallest accepted window edge in pixels.
pub const MIN_EDGE: u32 = 320;
/// Largest accepted window edge in pixels.
pub const MAX_EDGE: u32 = 16384;
/// UI scale limits.
pub const SCALE_RANGE: (f32, f32) = (0.5, 4.0);
/// Frame-rate caps the launcher offers (0 = no cap: vsync, if on, is the only limit).
pub const FPS_LIMITS: [u32; 5] = [0, 30, 60, 120, 144];
/// Accepted range of a non-zero frame-rate cap (a hand-edited file may hold any value in it).
pub const FPS_LIMIT_RANGE: (u32, u32) = (10, 1000);

/// Window / graphics settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowOptions {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    /// Multiplier for the launcher and in-window UI.
    pub ui_scale: f32,
    /// Draw a frames-per-second counter in the top-left corner of the game picture.
    pub show_fps: bool,
    /// Present at most this many frames per second (0 = no cap; with vsync on, the lower of the two applies).
    pub fps_limit: u32,
    /// Lift the game's own 30 FPS cap (it asks for every second refresh of a 60 Hz display); the game then draws as
    /// often as `fps_limit` and vsync allow. Its simulation runs on measured time, so it does not run faster.
    pub unlock_fps: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions {
            width: 1024,
            height: 768,
            fullscreen: false,
            vsync: true,
            ui_scale: 1.0,
            show_fps: false,
            fps_limit: 60,
            unlock_fps: false,
        }
    }
}

impl WindowOptions {
    /// Checks every field is in range.
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [("width", self.width), ("height", self.height)] {
            if !(MIN_EDGE..=MAX_EDGE).contains(&v) {
                return Err(format!("{name} {v} is outside {MIN_EDGE}..={MAX_EDGE}"));
            }
        }
        if !(SCALE_RANGE.0..=SCALE_RANGE.1).contains(&self.ui_scale) {
            return Err(format!("ui_scale {} is outside {}..={}", self.ui_scale, SCALE_RANGE.0, SCALE_RANGE.1));
        }
        if self.fps_limit != 0 && !(FPS_LIMIT_RANGE.0..=FPS_LIMIT_RANGE.1).contains(&self.fps_limit) {
            return Err(format!(
                "fps_limit {} is outside {}..={} (0 = no limit)",
                self.fps_limit, FPS_LIMIT_RANGE.0, FPS_LIMIT_RANGE.1
            ));
        }
        Ok(())
    }

    /// The `bhpc-input` mapper configuration for this window (the guest keeps its 1024x768 content space).
    pub fn input_config(&self) -> bhpc_input::Config {
        bhpc_input::Config { window: (self.width, self.height), ..bhpc_input::Config::default() }
    }
}
