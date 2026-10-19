//! [`LaunchConfig`]: everything `bhpc-run` needs, persisted as TOML.

use crate::keys::KeyBindings;
use crate::window::WindowOptions;
use bhpc_loader::apk::ApkError;
use bhpc_vfs::DataDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The launcher's result and `bhpc-run`'s input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// The user's own APK.
    pub apk: PathBuf,
    /// Root of the `bhpc_vfs::DataDirs` layout.
    pub data_dir: PathBuf,
    /// Run unrecognised builds anyway.
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub window: WindowOptions,
    #[serde(default)]
    pub keys: KeyBindings,
}

/// Launcher failures.
#[derive(Debug, PartialEq, Eq)]
pub enum LauncherError {
    Io(String),
    Parse(String),
    Apk(ApkError),
    Window(String),
    Conflict(String),
    NoDefaultDir,
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LauncherError::Io(e) => write!(f, "i/o error: {e}"),
            LauncherError::Parse(e) => write!(f, "bad config: {e}"),
            LauncherError::Apk(e) => write!(f, "{e}"),
            LauncherError::Window(e) => write!(f, "bad window option: {e}"),
            LauncherError::Conflict(e) => write!(f, "key bindings conflict: {e}"),
            LauncherError::NoDefaultDir => write!(f, "this platform has no default data directory"),
        }
    }
}

impl std::error::Error for LauncherError {}

impl LaunchConfig {
    /// A config with default options for `apk` and `data_dir`.
    pub fn new(apk: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        LaunchConfig {
            apk: apk.into(),
            data_dir: data_dir.into(),
            force: false,
            window: WindowOptions::default(),
            keys: KeyBindings::default(),
        }
    }

    /// The default data directory root (`bhpc_vfs::DataDirs::platform_default`).
    pub fn default_data_dir() -> Result<PathBuf, LauncherError> {
        DataDirs::platform_default().map(|d| d.root).ok_or(LauncherError::NoDefaultDir)
    }

    /// `<config dir>/blockheads-pc/launcher.toml`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("blockheads-pc").join("launcher.toml"))
    }

    /// The data directory layout.
    pub fn data_dirs(&self) -> DataDirs {
        DataDirs::new(&self.data_dir)
    }

    /// Serialises to TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("LaunchConfig always serialises")
    }

    /// Parses TOML (missing option tables take their defaults).
    pub fn from_toml(text: &str) -> Result<Self, LauncherError> {
        toml::from_str(text).map_err(|e| LauncherError::Parse(e.to_string()))
    }

    /// Reads `path`; `Ok(None)` when it does not exist yet (first run).
    pub fn load(path: &Path) -> Result<Option<Self>, LauncherError> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::from_toml(&t).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(LauncherError::Io(e.to_string())),
        }
    }

    /// Writes `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<(), LauncherError> {
        let io = |e: std::io::Error| LauncherError::Io(e.to_string());
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(""))).map_err(io)?;
        std::fs::write(path, self.to_toml()).map_err(io)
    }

    /// Checks window options, key conflicts and the APK. Returns the APK summary.
    pub fn validate(&self) -> Result<crate::apk::ApkInfo, LauncherError> {
        self.window.validate().map_err(LauncherError::Window)?;
        if let Some((a, b)) = self.keys.conflicts().first() {
            return Err(LauncherError::Conflict(format!(
                "{} and {} share {}",
                a.label(),
                b.label(),
                self.keys.get(*a)
            )));
        }
        crate::apk::validate(&self.apk, self.force).map_err(LauncherError::Apk)
    }

    /// Command line for `bhpc-run`: `--apk`, `--data-dir`, `--window WxH`, `--fps-limit N` (0 = none), `--unlock-fps`
    /// (always: the FPS limit is the only cap), `--force`, `--fullscreen`, `--no-vsync`, `--show-fps`, `--fps-position P` (when the counter is shown and not top-left), `--game-speed F` (when not 1) and a `--bind ACTION=BINDING` for every key binding that is not the
    /// default.
    pub fn run_args(&self) -> Vec<String> {
        let mut a = vec![
            "--apk".to_string(),
            self.apk.display().to_string(),
            "--data-dir".to_string(),
            self.data_dir.display().to_string(),
            "--window".to_string(),
            format!("{}x{}", self.window.width, self.window.height),
            "--fps-limit".to_string(),
            self.window.fps_limit.to_string(),
            "--unlock-fps".to_string(),
        ];
        for (on, flag) in [
            (self.force, "--force"),
            (self.window.fullscreen, "--fullscreen"),
            (!self.window.vsync, "--no-vsync"),
            (self.window.show_fps, "--show-fps"),
        ] {
            if on {
                a.push(flag.to_string());
            }
        }
        if self.window.show_fps && self.window.fps_position != crate::window::FpsPosition::TopLeft {
            a.extend(["--fps-position".to_string(), self.window.fps_position.flag().to_string()]);
        }
        if self.window.game_speed != 1.0 {
            a.extend(["--game-speed".to_string(), self.window.game_speed.to_string()]);
        }
        a.extend(self.keys.bind_flags());
        a
    }
}

/// Replaces `--config FILE` in a command line by the flags the file stands for. Arguments given next to it come
/// first, so they win over the file (`bhpc-run` reads the first occurrence of an option).
pub fn expand_args(args: &[String]) -> Result<Vec<String>, LauncherError> {
    let Some(i) = args.iter().position(|a| a == "--config") else {
        return Ok(args.to_vec());
    };
    let path = args.get(i + 1).ok_or_else(|| LauncherError::Io("--config needs a file".to_string()))?;
    let cfg = LaunchConfig::load(Path::new(path))?
        .ok_or_else(|| LauncherError::Io(format!("config file {path} does not exist")))?;
    let mut out: Vec<String> = args[..i].iter().chain(&args[i + 2..]).cloned().collect();
    out.extend(cfg.run_args());
    Ok(out)
}
