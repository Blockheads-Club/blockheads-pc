//! The launcher's update flow. Nothing here runs on its own: the page asks (`update_check`, shown as a notice) and
//! only a click on "install" calls `update_install`, which downloads the signed file the release feed names
//! (`plugins.updater` in tauri.conf.json), verifies its signature against the pinned key and installs it.
//!
//! Linux packages the updater cannot replace (deb, rpm: only an AppImage is a single file the app may overwrite)
//! still get the notice, with a link to the release page instead of the install button.

use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Where the releases (and so the manual downloads) live.
pub const RELEASES_URL: &str = "https://github.com/Blockheads-Club/blockheads-pc/releases";

/// What the page shows about an available update (`UpdateInfo` in app/src/lib/types.ts).
#[derive(Debug, Serialize, PartialEq)]
pub struct Info {
    pub version: String,
    /// 0 when the feed does not say.
    pub size_mb: u64,
    pub notes: Vec<String>,
    /// Set when this install cannot update itself: the release page to download the new package from.
    pub manual_url: Option<String>,
}

/// Download state the page polls (`UpdateProgress`).
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Progress {
    pub stage: &'static str,
    pub percent: u64,
    pub done_mb: f64,
    pub total_mb: f64,
    pub mb_per_s: f64,
    pub eta_s: u64,
}

impl Default for Progress {
    fn default() -> Self {
        Progress { stage: "downloading", percent: 0, done_mb: 0.0, total_mb: 0.0, mb_per_s: 0.0, eta_s: 0 }
    }
}

#[derive(Default)]
pub struct UpdateState {
    found: Mutex<Option<Update>>,
    progress: Mutex<Progress>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Linux outside an AppImage (deb, rpm, a build from source) cannot be replaced by the updater.
pub fn self_updating(os: &str, appimage: bool) -> bool {
    os != "linux" || appimage
}

pub fn release_page(version: &str) -> String {
    format!("{RELEASES_URL}/tag/v{version}")
}

/// Release notes as list items: one per non-empty line, list markers and bare links dropped (the feed's notes
/// field is a link to the release page by default).
pub fn notes_lines(body: Option<&str>) -> Vec<String> {
    body.unwrap_or_default()
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*']).trim())
        .filter(|l| !l.is_empty() && !l.starts_with("http"))
        .map(str::to_string)
        .collect()
}

/// Size of the file the update downloads: the feed's platform entry may carry `size` (bytes) beside `url`.
pub fn size_mb(raw: &Value, url: &str) -> u64 {
    let bytes = raw["platforms"]
        .as_object()
        .into_iter()
        .flat_map(|p| p.values())
        .find(|p| p["url"].as_str() == Some(url))
        .and_then(|p| p["size"].as_u64())
        .unwrap_or(0);
    bytes.div_ceil(1_000_000)
}

/// Progress after `done` of `total` bytes (0 = unknown) in `secs` seconds.
pub fn progress(done: u64, total: u64, secs: f64) -> Progress {
    let mb = |b: u64| (b as f64 / 1e6 * 10.0).round() / 10.0;
    let rate = if secs > 0.0 { done as f64 / 1e6 / secs } else { 0.0 };
    let left = total.saturating_sub(done) as f64 / 1e6;
    Progress {
        stage: "downloading",
        percent: (done.min(total) * 100).checked_div(total).unwrap_or(0),
        done_mb: mb(done),
        total_mb: mb(total),
        mb_per_s: (rate * 10.0).round() / 10.0,
        eta_s: if rate > 0.0 && total > 0 { (left / rate).ceil() as u64 } else { 0 },
    }
}

fn info_for(u: &Update, os: &str, appimage: bool) -> Info {
    Info {
        version: u.version.clone(),
        size_mb: size_mb(&u.raw_json, u.download_url.as_str()),
        notes: notes_lines(u.body.as_deref()),
        manual_url: (!self_updating(os, appimage)).then(|| release_page(&u.version)),
    }
}

/// Asks the release feed. `Ok(None)` is "up to date".
#[tauri::command]
pub async fn update_check(app: AppHandle, state: State<'_, UpdateState>) -> Result<Option<Info>, String> {
    let found = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let info = found.as_ref().map(|u| info_for(u, std::env::consts::OS, std::env::var_os("APPIMAGE").is_some()));
    *lock(&state.found) = found;
    Ok(info)
}

/// Downloads and installs the update `update_check` found. Only the page's install button calls this.
#[tauri::command]
pub async fn update_install(state: State<'_, UpdateState>) -> Result<(), String> {
    let appimage = std::env::var_os("APPIMAGE").is_some();
    if !self_updating(std::env::consts::OS, appimage) {
        return Err("this install cannot update itself: download the new package from the release page".into());
    }
    let update = lock(&state.found).clone().ok_or("no update was found: check for updates first")?;
    *lock(&state.progress) = Progress::default();
    let (started, mut done) = (Instant::now(), 0u64);
    let st: &UpdateState = &state;
    let bytes = update
        .download(
            |chunk, total| {
                done += chunk as u64;
                *lock(&st.progress) = progress(done, total.unwrap_or(0), started.elapsed().as_secs_f64());
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    lock(&state.progress).stage = "installing";
    update.install(bytes).map_err(|e| e.to_string())?;
    lock(&state.progress).stage = "ready";
    Ok(())
}

#[tauri::command]
pub fn update_progress(state: State<'_, UpdateState>) -> Progress {
    lock(&state.progress).clone()
}

/// Registers the updater plugin and the state the commands share.
pub fn init<R: tauri::Runtime>(b: tauri::Builder<R>) -> tauri::Builder<R> {
    b.plugin(tauri_plugin_updater::Builder::new().build()).manage(UpdateState::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_appimage_replaces_itself_on_linux() {
        assert!(self_updating("windows", false));
        assert!(self_updating("macos", false));
        assert!(self_updating("linux", true));
        assert!(!self_updating("linux", false));
        assert_eq!(release_page("0.2.0"), "https://github.com/Blockheads-Club/blockheads-pc/releases/tag/v0.2.0");
    }

    #[test]
    fn notes_become_list_items() {
        assert_eq!(notes_lines(Some("- one\n\n* two\nthree")), ["one", "two", "three"]);
        assert!(notes_lines(Some("https://github.com/x/releases/tag/v1")).is_empty());
        assert!(notes_lines(None).is_empty());
    }

    #[test]
    fn size_comes_from_the_matching_platform() {
        let raw = json!({"platforms": {"a": {"url": "u1", "size": 1}, "b": {"url": "u2", "size": 43_500_001}, "c": {"url": "u3"}}});
        assert_eq!(size_mb(&raw, "u2"), 44);
        assert_eq!(size_mb(&raw, "u3"), 0);
        assert_eq!(size_mb(&raw, "none"), 0);
        assert_eq!(size_mb(&json!({}), "u"), 0);
    }

    #[test]
    fn progress_math() {
        let p = progress(18_000_000, 43_000_000, 10.0);
        assert_eq!((p.percent, p.done_mb, p.total_mb, p.mb_per_s, p.eta_s), (41, 18.0, 43.0, 1.8, 14));
        let unknown = progress(5_000_000, 0, 0.0);
        assert_eq!((unknown.percent, unknown.mb_per_s, unknown.eta_s), (0, 0.0, 0));
        assert_eq!(progress(9, 5, 1.0).percent, 100);
        assert_eq!(Progress::default().stage, "downloading");
    }
}
