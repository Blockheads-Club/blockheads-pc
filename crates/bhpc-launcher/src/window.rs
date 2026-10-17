//! Window and graphics options.

use serde::{Deserialize, Serialize};

/// Smallest accepted window edge in pixels.
pub const MIN_EDGE: u32 = 320;
/// Largest accepted window edge in pixels.
pub const MAX_EDGE: u32 = 16384;
/// UI scale limits.
pub const SCALE_RANGE: (f32, f32) = (0.5, 4.0);

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
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions { width: 1024, height: 768, fullscreen: false, vsync: true, ui_scale: 1.0 }
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
        Ok(())
    }

    /// The `bhpc-input` mapper configuration for this window (the guest keeps its 1024x768 content space).
    pub fn input_config(&self) -> bhpc_input::Config {
        bhpc_input::Config { window: (self.width, self.height), ..bhpc_input::Config::default() }
    }
}
