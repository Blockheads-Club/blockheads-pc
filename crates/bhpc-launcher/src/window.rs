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
/// Game speeds the launcher offers (1 = normal; see `bhpc-run`'s `gamespeed`).
pub const GAME_SPEEDS: [f64; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0];
/// Accepted range of the game speed (a hand-edited file may hold any value in it).
pub const GAME_SPEED_RANGE: (f64, f64) = (0.25, 4.0);

/// Corner of the game picture the FPS counter sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FpsPosition {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl FpsPosition {
    /// The `bhpc-run --fps-position` value.
    pub fn flag(self) -> &'static str {
        match self {
            FpsPosition::TopLeft => "tl",
            FpsPosition::TopRight => "tr",
            FpsPosition::BottomLeft => "bl",
            FpsPosition::BottomRight => "br",
        }
    }
}

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
    /// Corner of the game picture the counter sits in.
    pub fps_position: FpsPosition,
    /// Present at most this many frames per second (0 = no cap; with vsync on, the lower of the two applies). The
    /// game's own 30 FPS cap is always lifted (`--unlock-fps`), so this is the only cap; its simulation runs on
    /// measured time, so it does not run faster.
    pub fps_limit: u32,
    /// How fast the world runs compared to normal (1 = normal; single-player only, see `bhpc-run`'s `gamespeed`).
    pub game_speed: f64,
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
            fps_position: FpsPosition::TopLeft,
            fps_limit: 60,
            game_speed: 1.0,
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
        if !(GAME_SPEED_RANGE.0..=GAME_SPEED_RANGE.1).contains(&self.game_speed) {
            return Err(format!(
                "game_speed {} is outside {}..={}",
                self.game_speed, GAME_SPEED_RANGE.0, GAME_SPEED_RANGE.1
            ));
        }
        Ok(())
    }

    /// The `bhpc-input` mapper configuration for this window (the guest keeps its 1024x768 content space).
    pub fn input_config(&self) -> bhpc_input::Config {
        bhpc_input::Config { window: (self.width, self.height), ..bhpc_input::Config::default() }
    }
}
