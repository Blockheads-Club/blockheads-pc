//! The operations the Tauri commands perform, kept here so they are tested like the rest of the launcher logic.

use crate::config::{LaunchConfig, LauncherError};
use crate::datadir;
use crate::import::{self, Existing, ImportSummary};
use crate::prefs::{Paths, Prefs};
use crate::session::sidecar_path;
use crate::{apk, keys::Binding};
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What the UI needs at start-up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Boot {
    /// `None` until setup has finished.
    pub config: Option<LaunchConfig>,
    /// A fresh configuration with the platform defaults (used as the setup draft and for "reset").
    pub defaults: LaunchConfig,
    pub prefs: Prefs,
    /// The preferences were saved before: `false` on the very first start, when the UI picks the language from the
    /// system's.
    pub prefs_saved: bool,
}

/// The launcher's config directory: `BHPC_CONFIG_DIR` when set (tests, portable installs), else the platform's.
pub fn paths_from(env: Option<OsString>, platform: Option<Paths>) -> Result<Paths, LauncherError> {
    match env {
        Some(dir) if !dir.is_empty() => Ok(Paths::new(PathBuf::from(dir))),
        _ => platform.ok_or(LauncherError::NoDefaultDir),
    }
}

/// [`paths_from`] with the process environment.
pub fn paths() -> Result<Paths, LauncherError> {
    paths_from(std::env::var_os("BHPC_CONFIG_DIR"), Paths::platform_default())
}

/// The default data directory: `BHPC_DATA_DIR` when set, else the platform's.
pub fn data_dir_from(
    env: Option<OsString>,
    platform: Result<PathBuf, LauncherError>,
) -> Result<PathBuf, LauncherError> {
    match env {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => platform,
    }
}

/// Loads everything the UI starts from.
pub fn boot(paths: &Paths, default_data_dir: PathBuf) -> Result<Boot, LauncherError> {
    Ok(Boot {
        config: LaunchConfig::load(&paths.config())?,
        defaults: LaunchConfig::new("", default_data_dir),
        prefs: Prefs::load(&paths.prefs())?,
        prefs_saved: paths.prefs().exists(),
    })
}

/// Saves a configuration edited in Settings (window options must be valid; key conflicts are allowed and shown).
pub fn save_config(paths: &Paths, config: LaunchConfig) -> Result<LaunchConfig, LauncherError> {
    config.window.validate().map_err(LauncherError::Window)?;
    config.save(&paths.config())?;
    Ok(config)
}

/// Saves the preferences.
pub fn save_prefs(paths: &Paths, prefs: Prefs) -> Result<Prefs, LauncherError> {
    let prefs = prefs.sanitized();
    prefs.save(&paths.prefs())?;
    Ok(prefs)
}

/// Points the configuration at `apk_path` (checked first), keeping a private copy when asked.
fn adopt_apk(config: &mut LaunchConfig, apk_path: &Path, keep_copy: bool) -> Result<(), LauncherError> {
    apk::validate(apk_path, config.force).map_err(LauncherError::Apk)?;
    config.apk = if keep_copy {
        datadir::keep_private_copy(apk_path, &config.data_dir).map_err(|e| LauncherError::Io(e.to_string()))?
    } else {
        apk_path.to_path_buf()
    };
    Ok(())
}

/// Ends setup: validates and saves the configuration and the preferences.
pub fn finish_setup(paths: &Paths, mut config: LaunchConfig, prefs: Prefs) -> Result<LaunchConfig, LauncherError> {
    config.window.validate().map_err(LauncherError::Window)?;
    let apk_path = config.apk.clone();
    adopt_apk(&mut config, &apk_path, prefs.keep_private_copy)?;
    config.save(&paths.config())?;
    save_prefs(paths, prefs)?;
    Ok(config)
}

/// Settings: switch to another game file.
pub fn use_apk(paths: &Paths, apk_path: &Path, force: bool, keep_copy: bool) -> Result<LaunchConfig, LauncherError> {
    let mut config =
        LaunchConfig::load(&paths.config())?.ok_or_else(|| LauncherError::Io("no configuration yet".into()))?;
    config.force = force;
    adopt_apk(&mut config, apk_path, keep_copy)?;
    config.save(&paths.config())?;
    Ok(config)
}

/// Checks and normalises binding text typed or captured in the UI (`ctrl + left` becomes `Ctrl+Left`).
pub fn normalize_binding(text: &str) -> Result<String, LauncherError> {
    Binding::parse(text).map(|b| b.to_string()).map_err(|e| LauncherError::Parse(e.to_string()))
}

/// Imports saves into `data_dir`.
pub fn import_saves(source: &Path, overwrite: bool, data_dir: &Path) -> Result<ImportSummary, LauncherError> {
    let dirs = bhpc_vfs::DataDirs::new(data_dir);
    let policy = if overwrite { Existing::Overwrite } else { Existing::Skip };
    let report = import::import_saves(source, &dirs, policy).map_err(|e| LauncherError::Io(e.to_string()))?;
    Ok(report.summary(&dirs))
}

/// Settings > Move: copies the data folder to `to` and points the configuration at it. The old folder stays.
pub fn move_data_dir(paths: &Paths, to: &Path) -> Result<LaunchConfig, LauncherError> {
    let mut config =
        LaunchConfig::load(&paths.config())?.ok_or_else(|| LauncherError::Io("no configuration yet".into()))?;
    if config.data_dir != to {
        datadir::copy_dir(&config.data_dir, to).map_err(|e| LauncherError::Io(e.to_string()))?;
        if let Some(rel) = config.apk.strip_prefix(&config.data_dir).ok().map(Path::to_path_buf) {
            config.apk = to.join(rel);
        }
        config.data_dir = to.to_path_buf();
        config.save(&paths.config())?;
    }
    Ok(config)
}

/// A process to start: program, arguments and extra environment.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Window title of the game: "Blockheads PC · Playing The Blockheads 1.7.4".
    pub title: String,
    /// The game window size and fullscreen setting, for the view inside the launcher.
    pub window: crate::window::WindowOptions,
    /// The key bindings (the launcher itself handles the fullscreen one in the in-window view).
    pub keys: crate::keys::KeyBindings,
    /// A link to hand the game once it runs (`--open-uri`, see [`open_uri_arg`]).
    pub open_uri: Option<String>,
}

impl LaunchPlan {
    /// The arguments for a game in a window of its own, or with `embed` (a frame file) for the launcher's window.
    pub fn args_for(&self, embed: Option<&Path>) -> Vec<String> {
        let mut a = self.args.clone();
        // Start-up progress lines for the loading screen (`--embed` implies them).
        a.push("--progress".to_string());
        if let Some(f) = embed {
            a.extend(["--embed".to_string(), f.display().to_string()]);
        }
        if let Some(u) = &self.open_uri {
            a.extend(["--open-uri".to_string(), u.clone()]);
        }
        a
    }
}

/// The link the launcher was started with: `--open-uri URL`, or a bare `http(s)://` or `blockheads://` argument (what
/// a desktop file's `%u` passes). The game gets it at its next start (`bhpc-run --open-uri`, `VerdeActivity.nativeHandleUri`), e.g. a
/// `http://theblockheads.net/join.php?...` join link.
pub fn open_uri_arg(args: &[String]) -> Option<String> {
    if let Some(i) = args.iter().position(|a| a == "--open-uri") {
        return args.get(i + 1).cloned();
    }
    args.iter().find(|a| ["http://", "https://", "blockheads://"].iter().any(|p| a.starts_with(p))).cloned()
}

/// The game window's title in `lang` (one of [`crate::prefs::LANGUAGES`], else English) for the game `version`
/// (`None`: a build the launcher does not know). The same texts as the UI's `tb.playing` and `apk.gameUnknown`.
pub fn game_title(lang: &str, version: Option<&str>) -> String {
    let (playing, unknown) = match lang {
        "tr" => ("The Blockheads {v} oynanıyor", "(bilinmeyen sürüm)"),
        "ru" => ("Идёт игра: The Blockheads {v}", "(неизвестная сборка)"),
        "kk" => ("Ойын жүріп жатыр: The Blockheads {v}", "(белгісіз жинақ)"),
        _ => ("Playing The Blockheads {v}", "(unknown build)"),
    };
    format!("Blockheads PC · {}", playing.replace("{v}", version.unwrap_or(unknown)))
}

/// What to run for "Play": the sidecar next to the launcher with `--config <launcher.toml>`. The game file is
/// checked first so a missing or changed APK is reported here rather than as a crash.
pub fn launch_plan(paths: &Paths, exe_dir: &Path) -> Result<LaunchPlan, LauncherError> {
    let config =
        LaunchConfig::load(&paths.config())?.ok_or_else(|| LauncherError::Io("setup is not finished".into()))?;
    config.window.validate().map_err(LauncherError::Window)?;
    let info = apk::validate(&config.apk, config.force).map_err(LauncherError::Apk)?;
    let prefs = Prefs::load(&paths.prefs())?;
    let title = game_title(&prefs.language, info.release.map(bhpc_loader::apk::Release::version));
    let mut env = Vec::new();
    if !prefs.game_sound {
        env.push(("SDL_AUDIODRIVER".to_string(), "dummy".to_string()));
    }
    Ok(LaunchPlan {
        program: sidecar_path(exe_dir),
        args: vec![
            "--config".to_string(),
            paths.config().display().to_string(),
            "--title".to_string(),
            title.clone(),
            "--lang".to_string(),
            prefs.language.clone(),
        ],
        env,
        title,
        window: config.window,
        keys: config.keys,
        open_uri: None,
    })
}

/// Where the client mods live: below the configured data folder, else below `default_data_dir` (before setup).
/// A broken configuration also falls back to the default.
pub fn mods_dirs(paths: &Paths, default_data_dir: PathBuf) -> crate::mods::ModsDirs {
    let data = LaunchConfig::load(&paths.config()).ok().flatten().map_or(default_data_dir, |c| c.data_dir);
    crate::mods::ModsDirs::new(&data)
}
