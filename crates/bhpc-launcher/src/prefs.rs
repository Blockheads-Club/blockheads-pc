//! Launcher preferences (everything that is not part of the game's [`crate::LaunchConfig`]) and where the
//! launcher keeps its files.

use crate::config::LauncherError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Languages the launcher has strings for.
pub const LANGUAGES: [&str; 4] = ["en", "tr", "ru", "kk"];
/// Theme choices.
pub const THEMES: [&str; 3] = ["system", "light", "dark"];

/// Launcher preferences, saved next to `launcher.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// One of [`LANGUAGES`].
    pub language: String,
    /// One of [`THEMES`].
    pub theme: String,
    /// Ask once a week whether a new launcher version exists.
    pub auto_update_check: bool,
    /// Copy the APK into the data folder so the game survives the original being moved.
    pub keep_private_copy: bool,
    /// Game sound on or off.
    pub game_sound: bool,
    /// Mute the game while the launcher is in the background.
    pub mute_in_background: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            language: "en".to_string(),
            theme: "system".to_string(),
            auto_update_check: true,
            keep_private_copy: true,
            game_sound: true,
            mute_in_background: false,
        }
    }
}

impl Prefs {
    /// Replaces values the launcher does not know by their defaults.
    pub fn sanitized(mut self) -> Self {
        let d = Prefs::default();
        if !LANGUAGES.contains(&self.language.as_str()) {
            self.language = d.language;
        }
        if !THEMES.contains(&self.theme.as_str()) {
            self.theme = d.theme;
        }
        self
    }

    /// Reads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Prefs, LauncherError> {
        match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str::<Prefs>(&t).map(Prefs::sanitized).map_err(|e| LauncherError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Prefs::default()),
            Err(e) => Err(LauncherError::Io(e.to_string())),
        }
    }

    /// Writes `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<(), LauncherError> {
        let io = |e: std::io::Error| LauncherError::Io(e.to_string());
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(""))).map_err(io)?;
        let text = toml::to_string_pretty(self).expect("Prefs always serialises");
        std::fs::write(path, text).map_err(io)
    }
}

/// The launcher's own files below one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    /// Files below `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Paths {
        Paths { dir: dir.into() }
    }

    /// `<config dir>/blockheads-pc`, `None` when the platform has no config directory.
    pub fn platform_default() -> Option<Paths> {
        dirs::config_dir().map(|d| Paths::new(d.join("blockheads-pc")))
    }

    /// The game configuration.
    pub fn config(&self) -> PathBuf {
        self.dir.join("launcher.toml")
    }

    /// The preferences.
    pub fn prefs(&self) -> PathBuf {
        self.dir.join("prefs.toml")
    }
}
