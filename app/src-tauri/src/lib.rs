//! Blockheads PC launcher: Tauri commands over `bhpc-launcher`. All logic lives in that crate (tested to 100 %);
//! this file only converts types and talks to the window system.

mod embed;
#[cfg(target_os = "linux")]
mod embed_gl;
mod mods;
mod tray;
mod updater;

use bhpc_launcher::apk::{self, ApkReport};
use bhpc_launcher::config::LaunchConfig;
use bhpc_launcher::import::ImportSummary;
use bhpc_launcher::ops::{self, Boot};
use bhpc_launcher::prefs::Prefs;
use bhpc_launcher::session::{GameState, Session, Snapshot};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, RunEvent, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
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

/// The splash stays up at least this long, so it is seen rather than flashed.
pub const SPLASH_MIN: Duration = Duration::from_millis(700);

/// Minimum splash time: `BHPC_SPLASH_HOLD_MS` in milliseconds (0 = none), else [`SPLASH_MIN`].
fn splash_hold(var: Option<std::ffi::OsString>) -> Duration {
    var.and_then(|v| v.to_str().and_then(|s| s.trim().parse::<u64>().ok()))
        .map(Duration::from_millis)
        .unwrap_or(SPLASH_MIN)
}

/// The splash page has drawn (its first `splash_get`): the minimum splash time counts from then, not from the
/// app start, so the splash is seen for all of it.
fn splash_shown(started: &Mutex<Instant>, shown: &AtomicBool) {
    if !shown.swap(true, Ordering::SeqCst) {
        *started.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}

/// Shared state: the supervised game process and the splash hand-over.
struct App {
    session: Mutex<Session>,
    splash: Mutex<Option<SplashState>>,
    /// When the splash page first drew (app start until then), see [`splash_shown`].
    started: Mutex<Instant>,
    shown: AtomicBool,
    hold: Duration,
    /// The main window is on screen: closing the splash is no longer a request to quit.
    revealed: AtomicBool,
    /// The running game is shown inside the main window (see `embed`).
    embedded: AtomicBool,
    frames: AtomicU32,
    /// A link the launcher was started with, handed to the next game start (`--open-uri`).
    open_uri: Mutex<Option<String>>,
}

impl Default for App {
    fn default() -> Self {
        App {
            session: Mutex::default(),
            splash: Mutex::default(),
            started: Mutex::new(Instant::now()),
            shown: AtomicBool::new(false),
            hold: splash_hold(std::env::var_os("BHPC_SPLASH_HOLD_MS")),
            revealed: AtomicBool::new(false),
            embedded: AtomicBool::new(false),
            frames: AtomicU32::new(0),
            open_uri: Mutex::new(ops::open_uri_arg(&std::env::args().skip(1).collect::<Vec<_>>())),
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
    /// The update notice is wired to the release feed (see `updater`).
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
        updates_enabled: true,
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
    splash_shown(&state.started, &state.shown);
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
    let wait = state.hold.saturating_sub(state.started.lock().unwrap_or_else(|e| e.into_inner()).elapsed());
    if !wait.is_zero() {
        tauri::async_runtime::spawn_blocking(move || std::thread::sleep(wait)).await.map_err(text)?;
    }
    state.revealed.store(true, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("main") {
        if embed::supported() {
            let _ = embed::prepare(&w).map_err(|e| eprintln!("in-window game view: {e}"));
        }
        w.show().map_err(text)?;
        // Frameless windows on Wayland drop the config's size hints when GTK realizes them; set them again.
        let _ = w.set_min_size(Some(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1)));
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

/// Height of the title bar plus the control strip above the game stage (CSS pixels; `--titlebar-h` and the strip).
const CHROME_HEIGHT: f64 = 84.0;

/// How the game runs: in the launcher's window or in one of its own.
#[derive(Serialize)]
struct Started {
    embedded: bool,
}

fn start_game(app: &App, window: &WebviewWindow, handle: &AppHandle, allow_embed: bool) -> Res<Started> {
    start_game_joining(app, window, handle, allow_embed, None)
}

/// Starts the game; with `join` it joins that server (`host:port`) from the main menu.
fn start_game_joining(
    app: &App,
    window: &WebviewWindow,
    handle: &AppHandle,
    allow_embed: bool,
    join: Option<String>,
) -> Res<Started> {
    let exe = std::env::current_exe().map_err(text)?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    let paths = ops::paths().map_err(text)?;
    let mut plan = ops::launch_plan(&paths, dir).map_err(text)?;
    plan.open_uri = app.open_uri.lock().unwrap_or_else(|e| e.into_inner()).take();
    plan.join = join;
    app.session().set_history(bhpc_launcher::progress::history_path(&paths.dir));
    let program = sidecar(plan.program.clone());
    // The frame file must exist before the game starts: it opens it at once.
    let frame = (allow_embed && embed::supported())
        .then(|| {
            let n = app.frames.fetch_add(1, Ordering::SeqCst);
            let dir = bhpc_launcher::embed::frame_dir();
            bhpc_launcher::embed::clean_stale(&dir, &bhpc_launcher::embed::pid_alive);
            let path = bhpc_launcher::embed::frame_path(&dir, n);
            bhpc_launcher::embed::FrameReader::create(&path)
                .map(|r| (r, path))
                .map_err(|e| eprintln!("frame file: {e}"))
        })
        .and_then(Result::ok);
    let args = plan.args_for(frame.as_ref().map(|(_, p)| p.as_path()));
    app.session().start(&program, &args, &plan.env).map_err(text)?;
    let Some((reader, _)) = frame else {
        app.embedded.store(false, Ordering::SeqCst);
        return Ok(Started { embedded: false });
    };
    let opened = app.session().input().ok_or_else(|| "the game has no input".to_string()).and_then(|input| {
        embed::open(
            window,
            embed::Opts { frame: reader, input, fullscreen_key: plan.keys.fullscreen.trigger(), app: handle.clone() },
        )
    });
    if let Err(e) = opened {
        // No view: run the game in a window of its own instead.
        eprintln!("in-window game view unavailable ({e}); using a separate game window");
        app.session().stop();
        app.session().start(&program, &plan.args_for(None), &plan.env).map_err(text)?;
        app.embedded.store(false, Ordering::SeqCst);
        return Ok(Started { embedded: false });
    }
    app.embedded.store(true, Ordering::SeqCst);
    if !plan.window.fullscreen && !window.is_maximized().unwrap_or(false) {
        fit_window(window, plan.window.width, plan.window.height);
    }
    if plan.window.fullscreen {
        let _ = embed::set_fullscreen(window, handle, true);
    }
    Ok(Started { embedded: true })
}

/// Size the launcher window so its game stage is `width` x `height` (logical pixels; the window's minimum size and
/// the screen may keep it bigger or smaller).
fn fit_window(window: &WebviewWindow, width: u32, height: u32) {
    let _ = window.set_size(tauri::LogicalSize::new(f64::from(width), f64::from(height) + CHROME_HEIGHT));
}

/// Settings: a window size was picked. The launcher window takes it at once (leaving maximized); fullscreen stays.
#[tauri::command]
fn window_resize(window: WebviewWindow, width: u32, height: u32) {
    if window.is_fullscreen().unwrap_or(false) {
        return;
    }
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    fit_window(&window, width, height);
}

/// Takes the in-window view away (the game ended) and leaves fullscreen.
fn end_view(app: &App, window: &WebviewWindow, handle: &AppHandle) {
    if app.embedded.swap(false, Ordering::SeqCst) {
        let _ = embed::close(window);
        if window.is_fullscreen().unwrap_or(false) {
            let _ = embed::set_fullscreen(window, handle, false);
        }
    }
}

#[tauri::command]
fn game_start(app: State<'_, App>, window: WebviewWindow, handle: AppHandle) -> Res<Started> {
    start_game(&app, &window, &handle, true)
}

/// Developer mode: the running game reloads its folder mods.
#[tauri::command]
fn mods_dev_reload(app: State<'_, App>, path: String) -> Result<(), bhpc_launcher::mods::ModError> {
    let _ = path;
    let input = app.session().input();
    bhpc_launcher::mods::dev::reload(input.as_ref())
}

/// Developer mode: a number that changes when a file in one of the folders changes (live reload).
#[tauri::command]
fn mods_dev_fingerprint(paths: Vec<String>) -> String {
    let all = paths.iter().fold(0u64, |a, p| a.rotate_left(7) ^ bhpc_launcher::mods::dev::fingerprint(Path::new(p)));
    all.to_string()
}

/// Developer mode: start the game and join a server (the local one).
#[tauri::command]
fn mods_dev_join(app: State<'_, App>, window: WebviewWindow, handle: AppHandle, address: String) -> Res<Started> {
    start_game_joining(&app, &window, &handle, true, Some(address))
}

#[tauri::command]
fn game_stop(app: State<'_, App>, window: WebviewWindow, handle: AppHandle) {
    app.session().stop();
    end_view(&app, &window, &handle);
}

#[tauri::command]
fn game_poll(app: State<'_, App>, window: WebviewWindow, handle: AppHandle, since: usize) -> Polled {
    let present = ops::paths()
        .ok()
        .and_then(|p| LaunchConfig::load(&p.config()).ok().flatten())
        .map(|c| c.apk.exists())
        .unwrap_or(true);
    let mut snap = app.session().poll(since, present);
    if app.embedded.load(Ordering::SeqCst) && !matches!(snap.state, GameState::Starting | GameState::Running) {
        end_view(&app, &window, &handle);
        // The runner could not render without a window: start it again with a window of its own.
        if snap.exit_code == Some(bhpc_launcher::embed::EMBED_UNAVAILABLE_EXIT)
            && start_game(&app, &window, &handle, false).is_ok()
        {
            snap = app.session().poll(0, present);
        }
    }
    Polled { snapshot: snap, embedded: app.embedded.load(Ordering::SeqCst) }
}

/// The poll result plus how the game is shown.
#[derive(Serialize)]
struct Polled {
    #[serde(flatten)]
    snapshot: Snapshot,
    embedded: bool,
}

/// The page tells where the game stage starts (see `embed::layout`).
#[tauri::command]
fn embed_layout(window: WebviewWindow, top: i32) -> Res<()> {
    embed::layout(&window, top)
}

/// The page's loading screen is done: the in-window game may fade in (see `embed::reveal`).
#[tauri::command]
fn embed_reveal(window: WebviewWindow) -> Res<()> {
    embed::reveal(&window)
}

#[tauri::command]
fn embed_fullscreen(window: WebviewWindow, handle: AppHandle, on: bool) -> Res<()> {
    embed::set_fullscreen(&window, &handle, on)
}

#[tauri::command]
fn save_log(app: State<'_, App>, path: String) -> Res<()> {
    app.session().save_log(Path::new(&path)).map_err(text)
}

/// Smallest launcher window (logical pixels): the size it opens at, as minWidth/minHeight in tauri.conf.json.
const MIN_SIZE: (f64, f64) = (1100.0, 760.0);

pub fn run() {
    let app = updater::init(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(App::default())
        .manage(mods::Pending::default())
        .setup(|app| {
            // The main window is created hidden (tauri.conf.json) and runs the boot work; `main_ready` shows it
            // (GTK warns when the frameless window gets its title bar after it was realized and shown early).
            // Only the splash is on screen until then. Should the splash fail to open, show the main window.
            tray::create(app.handle());
            // Text the in-window game copies arrives as `@copy` lines; the clipboard lives on the GTK main thread.
            let h = app.handle().clone();
            app.state::<App>().session().set_copy_handler(move |t| {
                let _ = h.run_on_main_thread(move || embed::copy_text(&t));
            });
            if let Err(e) = open_splash(app) {
                eprintln!("splash window failed: {e}");
                if let Some(w) = app.get_webview_window("main") {
                    w.show()?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            tray::on_close(window, event);
            // Closing the splash (Alt+F4) before the launcher is up means the user wants out.
            let closing = window.label() == "splash" && matches!(event, WindowEvent::CloseRequested { .. });
            if closing && !window.state::<App>().revealed.load(Ordering::SeqCst) {
                window.app_handle().exit(0);
            }
            // Some compositors ignore the min-size hint of a frameless window: grow it back.
            if let (WindowEvent::Resized(size), "main") = (event, window.label()) {
                let scale = window.scale_factor().unwrap_or(1.0);
                let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
                let maximized = window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false);
                if !maximized && size.width > 0 && (w < MIN_SIZE.0 || h < MIN_SIZE.1) {
                    let _ = window.set_size(LogicalSize::new(w.max(MIN_SIZE.0), h.max(MIN_SIZE.1)));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            splash_set,
            splash_get,
            splash_retry,
            app_quit,
            tray::tray_labels,
            main_ready,
            boot,
            save_config,
            save_prefs,
            finish_setup,
            check_apk,
            use_apk,
            normalize_binding,
            display_info,
            window_resize,
            import_saves,
            data_size,
            move_data_dir,
            open_path,
            game_start,
            game_stop,
            game_poll,
            embed_layout,
            embed_reveal,
            embed_fullscreen,
            save_log,
            updater::update_check,
            updater::update_install,
            updater::update_progress,
            updater::update_restart,
            mods::mods_overview,
            mods::mods_index,
            mods::mods_plan,
            mods::mods_inspect_file,
            mods::mods_inspect_link,
            mods::mods_install,
            mods::mods_set_enabled,
            mods::mods_set_grant,
            mods::mods_remove,
            mods::mods_auto_update,
            mods::mods_servers,
            mods::mods_clear_server,
            mods::mods_clear_servers,
            mods::mods_dev_inspect,
            mods_dev_reload,
            mods_dev_fingerprint,
            mods_dev_join
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

    #[test]
    fn the_splash_time_counts_from_the_first_draw() {
        let (started, shown) = (Mutex::new(Instant::now()), AtomicBool::new(false));
        std::thread::sleep(Duration::from_millis(30));
        splash_shown(&started, &shown);
        let first = *started.lock().unwrap();
        assert!(first.elapsed() < Duration::from_millis(30));
        std::thread::sleep(Duration::from_millis(5));
        splash_shown(&started, &shown);
        assert_eq!(*started.lock().unwrap(), first, "later calls keep the first draw");
    }
    use super::*;

    #[test]
    fn splash_hold_parses_milliseconds() {
        assert_eq!(splash_hold(None), SPLASH_MIN);
        assert_eq!(splash_hold(Some("".into())), SPLASH_MIN);
        assert_eq!(splash_hold(Some("soon".into())), SPLASH_MIN);
        assert_eq!(splash_hold(Some("0".into())), Duration::ZERO);
        assert_eq!(splash_hold(Some(" 2500 ".into())), Duration::from_millis(2500));
    }
}
