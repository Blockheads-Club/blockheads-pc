//! Launcher logic for the PC port (PLAN M9). Everything here is pure and testable; the egui front end
//! (`gui` feature, `src/gui.rs`) only draws [`wizard::Wizard`] and forwards input.
//!
//! * [`apk`]: validate the user's APK against the known `libApplication.so` hashes.
//! * [`keys`]: actions and bindings matching the `bhpc-input` mappings, parsed from / printed as text.
//! * [`window`]: resolution, fullscreen, vsync and UI scale.
//! * [`config`]: [`LaunchConfig`], the TOML-persisted result that `bhpc-run` consumes.
//! * [`import`]: Android save folder or zip into the `bhpc-vfs` data dir layout.
//! * [`wizard`]: the first-run step state machine.

pub mod apk;
pub mod config;
pub mod import;
pub mod keys;
pub mod window;
pub mod wizard;

pub use config::{LaunchConfig, LauncherError};

#[cfg(test)]
mod tests;
