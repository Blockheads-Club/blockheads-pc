//! Launcher logic for the PC port (PLAN M9). Everything here is testable without a display; the Tauri app
//! (`app/src-tauri`) is a thin command layer over it and `app/` draws the screens.
//!
//! * [`apk`]: validate the user's APK against the known `libApplication.so` hashes.
//! * [`keys`]: actions and bindings matching the `bhpc-input` mappings, parsed from / printed as text.
//! * [`window`]: resolution, fullscreen, vsync and UI scale.
//! * [`config`]: [`LaunchConfig`], the TOML-persisted result that `bhpc-run` consumes.
//! * [`import`]: Android save folder or zip into the `bhpc-vfs` data dir layout.
//! * [`wizard`]: the first-run step state machine.
//! * [`prefs`]: launcher preferences (language, theme, ...) and the config directory layout.
//! * [`datadir`]: data folder size and copy.
//! * [`ops`]: the operations behind the app's commands.
//! * [`embed`]: playing inside the launcher window (frame file, input commands).
//! * [`session`]: spawn and supervise `bhpc-run`, keep its log.

pub mod apk;
pub mod config;
pub mod datadir;
pub mod embed;
pub mod import;
pub mod keys;
pub mod ops;
pub mod prefs;
pub mod session;
pub mod window;
pub mod wizard;

pub use config::{LaunchConfig, LauncherError};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_app;
