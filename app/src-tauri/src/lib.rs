//! Blockheads PC launcher: Tauri commands over `bhpc-launcher`. All logic lives in that crate (tested to 100 %);
//! this file only converts types and talks to the window system.

use bhpc_launcher::apk::{self, ApkReport};
use bhpc_launcher::config::LaunchConfig;
use bhpc_launcher::import::ImportSummary;
use bhpc_launcher::ops::{self, Boot};
use bhpc_launcher::prefs::Prefs;
use bhpc_launcher::session::{Session, Snapshot};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

type Res<T> = Result<T, String>;

fn text<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// What the splash window shows; the hidden main window (which runs the boot work) pushes it.
#[derive(Clone, Default, Serialize, Deserialize)]
struct SplashState {
    boot: String,
    error: String,
    first_run: bool,
    version: String,
    config_dir: String,
    lang: String,
}

/// Minimum splash time from `BHPC_SPLASH_HOLD_MS` (milliseconds; unset, empty or invalid means none).
fn splash_hold(var: Option<std::ffi::OsString>) -> Duration {
    var.and_then(|v| v.to_str().and_then(|s| s.trim().parse::<u64>().ok()))
        .map(Duration::from_millis)
        .unwrap_or_default()
}

/// Shared state: the supervised game process and the splash hand-over.
struct App {
    session: Mutex<Session>,
    splash: Mutex<Option<SplashState>>,
    started: Instant,
    hold: Duration,
    /// The main window is on screen: closing the splash is no longer a request to quit.
    revealed: AtomicBool,
}

impl Default for App {
    fn default() -> Self {
        App {
            session: Mutex::default(),
            splash: Mutex::default(),
            started: Instant::now(),
            hold: splash_hold(std::env::var_os("BHPC_SPLASH_HOLD_MS")),
            revealed: AtomicBool::new(false),
        }
    }
}

impl App {
    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    config_dir: String,
    /// No release feed exists yet: the update UI stays off in real builds (see `update_check`).
    updates_enabled: bool,
    /// macOS draws the traffic lights (overlay title bar).
    native_controls: bool,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: match std::env::consts::OS {
            "macos" => "macos",
            "windows" => "windows",
            _ => "linux",
        },
        arch: std::env::consts::ARCH,
        config_dir: ops::paths().map(|p| p.dir.display().to_string()).unwrap_or_default(),
        updates_enabled: false,
        native_controls: cfg!(target_os = "macos"),
    }
}

/// The main window keeps this up to date while it boots; the splash window renders it.
#[tauri::command]
fn splash_set(app: AppHandle, state: State<'_, App>, splash: SplashState) {
    *state.splash.lock().unwrap_or_else(|e| e.into_inner()) = Some(splash.clone());
    let _ = app.emit_to("splash", "splash-state", splash);
}

#[tauri::command]
fn splash_get(state: State<'_, App>) -> Option<SplashState> {
    state.splash.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The splash's Retry button: the main window owns the boot work, so it is told to run it again.
#[tauri::command]
fn splash_retry(app: AppHandle) {
    let _ = app.emit_to("main", "splash-retry", ());
}

#[tauri::command]
fn app_quit(app: AppHandle) {
    app.exit(0);
}

/// The boot work is done: after the minimum splash time show the main window and close the splash.
#[tauri::command]
async fn main_ready(app: AppHandle, state: State<'_, App>) -> Res<()> {
    let wait = state.hold.saturating_sub(state.started.elapsed());
    if !wait.is_zero() {
        tauri::async_runtime::spawn_blocking(move || std::thread::sleep(wait)).await.map_err(text)?;
    }
    state.revealed.store(true, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("main") {
        w.show().map_err(text)?;
        let _ = w.set_focus();
    }
    if let Some(w) = app.get_webview_window("splash") {
        let _ = w.close();
    }
    Ok(())
}

/// The small frameless splash window (card size of the design). Transparent so the card's rounded corners show;
/// macOS cannot do that without the private API, so it gets a plain undecorated window there.
fn open_splash(app: &tauri::App) -> tauri::Result<()> {
    let b = WebviewWindowBuilder::new(app, "splash", WebviewUrl::App("splash".into()))
        .title("Blockheads PC")
        .inner_size(600.0, 380.0)
        .center()
        .decorations(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .shadow(false)
        .skip_taskbar(true)
        // Hidden until built: GTK warns when the frameless window gets its title bar after it was shown.
        .visible(false)
        .focused(true);
    #[cfg(not(target_os = "macos"))]
    let b = b.transparent(true);
    b.build()?.show()?;
    Ok(())
}

#[tauri::command]
fn boot() -> Res<Boot> {
    let data = ops::data_dir_from(std::env::var_os("BHPC_DATA_DIR"), LaunchConfig::default_data_dir()).map_err(text)?;
    ops::boot(&ops::paths().map_err(text)?, data).map_err(text)
}

#[tauri::command]
fn save_config(config: LaunchConfig) -> Res<LaunchConfig> {
    ops::save_config(&ops::paths().map_err(text)?, config).map_err(text)
}

#[tauri::command]
fn save_prefs(prefs: Prefs) -> Res<Prefs> {
    ops::save_prefs(&ops::paths().map_err(text)?, prefs).map_err(text)
}

#[tauri::command]
fn finish_setup(config: LaunchConfig, prefs: Prefs) -> Res<LaunchConfig> {
    ops::finish_setup(&ops::paths().map_err(text)?, config, prefs).map_err(text)
}

#[tauri::command]
async fn check_apk(path: String, force: bool) -> Res<ApkReport> {
    // Hashing a 60 MB archive: keep it off the UI thread.
    tauri::async_runtime::spawn_blocking(move || apk::check(Path::new(&path), force)).await.map_err(text)
}

#[tauri::command]
async fn use_apk(path: String, force: bool, keep_copy: bool) -> Res<LaunchConfig> {
    let paths = ops::paths().map_err(text)?;
    tauri::async_runtime::spawn_blocking(move || ops::use_apk(&paths, Path::new(&path), force, keep_copy).map_err(text))
        .await
        .map_err(text)?
}

#[tauri::command]
fn normalize_binding(text: String) -> Res<String> {
    ops::normalize_binding(&text).map_err(self::text)
}

#[derive(Serialize)]
struct Display {
    width: u32,
    height: u32,
    scale: f64,
}

#[tauri::command]
fn display_info(window: WebviewWindow) -> Option<Display> {
    let m = window.current_monitor().ok().flatten()?;
    Some(Display { width: m.size().width, height: m.size().height, scale: m.scale_factor() })
}

#[tauri::command]
async fn import_saves(source: String, overwrite: bool, data_dir: String) -> Res<ImportSummary> {
    tauri::async_runtime::spawn_blocking(move || {
        ops::import_saves(Path::new(&source), overwrite, Path::new(&data_dir)).map_err(text)
    })
    .await
    .map_err(text)?
}

#[tauri::command]
async fn data_size(path: String) -> u64 {
    tauri::async_runtime::spawn_blocking(move || bhpc_launcher::datadir::dir_size(Path::new(&path))).await.unwrap_or(0)
}

#[tauri::command]
async fn move_data_dir(to: String) -> Res<LaunchConfig> {
    let paths = ops::paths().map_err(text)?;
    tauri::async_runtime::spawn_blocking(move || ops::move_data_dir(&paths, Path::new(&to)).map_err(text))
        .await
        .map_err(text)?
}

#[tauri::command]
fn open_path(app: tauri::AppHandle, path: String) -> Res<()> {
    std::fs::create_dir_all(&path).map_err(text)?;
    app.opener().open_path(path, None::<&str>).map_err(text)
}

/// The sidecar sits next to this executable; `BHPC_RUN` points at another build (development).
fn sidecar(plan_program: PathBuf) -> PathBuf {
    std::env::var_os("BHPC_RUN").map(PathBuf::from).unwrap_or(plan_program)
}

#[tauri::command]
fn game_start(app: State<'_, App>) -> Res<()> {
    let exe = std::env::current_exe().map_err(text)?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    let plan = ops::launch_plan(&ops::paths().map_err(text)?, dir).map_err(text)?;
    app.session().start(&sidecar(plan.program), &plan.args, &plan.env).map_err(text)
}

#[tauri::command]
fn game_stop(app: State<'_, App>) {
    app.session().stop();
}

#[tauri::command]
fn game_poll(app: State<'_, App>, since: usize) -> Snapshot {
    let present = ops::paths()
        .ok()
        .and_then(|p| LaunchConfig::load(&p.config()).ok().flatten())
        .map(|c| c.apk.exists())
        .unwrap_or(true);
    app.session().poll(since, present)
}

#[tauri::command]
fn save_log(app: State<'_, App>, path: String) -> Res<()> {
    app.session().save_log(Path::new(&path)).map_err(text)
}

/// There is no release feed yet (the repository is not published): the UI hides the update flow in real builds
/// (`updates_enabled: false`) and these commands only answer that it is unavailable. The screens are exercised
/// against a mock backend in the e2e tests.
#[tauri::command]
fn update_check() -> Res<Option<()>> {
    Err("updates are not available in this build".to_string())
}

#[tauri::command]
fn update_install() -> Res<()> {
    Err("updates are not available in this build".to_string())
}

#[tauri::command]
fn update_progress() -> Res<()> {
    Err("updates are not available in this build".to_string())
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(App::default())
        .setup(|app| {
            // The main window is created hidden (tauri.conf.json) and runs the boot work; `main_ready` shows it
            // (GTK warns when the frameless window gets its title bar after it was realized and shown early).
            // Only the splash is on screen until then. Should the splash fail to open, show the main window.
            if let Err(e) = open_splash(app) {
                eprintln!("splash window failed: {e}");
                if let Some(w) = app.get_webview_window("main") {
                    w.show()?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the splash (Alt+F4) before the launcher is up means the user wants out.
            let closing = window.label() == "splash" && matches!(event, WindowEvent::CloseRequested { .. });
            if closing && !window.state::<App>().revealed.load(Ordering::SeqCst) {
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            splash_set,
            splash_get,
            splash_retry,
            app_quit,
            main_ready,
            boot,
            save_config,
            save_prefs,
            finish_setup,
            check_apk,
            use_apk,
            normalize_binding,
            display_info,
            import_saves,
            data_size,
            move_data_dir,
            open_path,
            game_start,
            game_stop,
            game_poll,
            save_log,
            update_check,
            update_install,
            update_progress
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Blockheads PC launcher");
    app.run(|handle, event| {
        // Never leave the game running behind a closed launcher.
        if let RunEvent::Exit = event {
            handle.state::<App>().session().stop();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_hold_parses_milliseconds() {
        assert_eq!(splash_hold(None), Duration::ZERO);
        assert_eq!(splash_hold(Some("".into())), Duration::ZERO);
        assert_eq!(splash_hold(Some("soon".into())), Duration::ZERO);
        assert_eq!(splash_hold(Some(" 2500 ".into())), Duration::from_millis(2500));
    }
}
