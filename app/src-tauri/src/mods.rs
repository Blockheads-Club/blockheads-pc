//! Tauri commands for client mods over `bhpc_launcher::mods` (all logic and its tests live there). This file adds
//! the HTTPS client, the checked packages waiting for "Install" and progress events.
//!
//! The in-game mod loader (milestone M6) is not here yet: installed mods are managed but do not run, and the
//! developer reload answers `needsLoader`.

use bhpc_launcher::config::LaunchConfig;
use bhpc_launcher::mods::cache::{self, CacheOverview};
use bhpc_launcher::mods::dev::{self, DevMod};
use bhpc_launcher::mods::index::{self, Browse, Fetch, IndexFile, Plan};
use bhpc_launcher::mods::install::{self, AutoUpdate, Candidate, Choices};
use bhpc_launcher::mods::{self, MRes, ModError, ModsDirs, Overview};
use bhpc_launcher::ops;
use bhpc_launcher::prefs::Prefs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// Checked files and links waiting for the player's "Install" (by SHA-256).
#[derive(Default)]
pub struct Pending(Mutex<HashMap<String, Candidate>>);

/// HTTPS through ureq (rustls with bundled roots).
struct Net;

impl Fetch for Net {
    fn get(&self, url: &str, limit: u64) -> MRes<Vec<u8>> {
        let resp = ureq::get(url)
            .timeout(std::time::Duration::from_secs(60))
            .set("User-Agent", concat!("blockheads-pc/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| ModError::with("download", e.to_string()))?;
        let mut buf = Vec::new();
        resp.into_reader()
            .take(limit + 1)
            .read_to_end(&mut buf)
            .map_err(|e| ModError::with("download", e.to_string()))?;
        Ok(buf)
    }
}

fn io(e: impl std::fmt::Display) -> ModError {
    ModError::with("io", e.to_string())
}

fn dirs() -> MRes<ModsDirs> {
    let paths = ops::paths().map_err(io)?;
    let default =
        ops::data_dir_from(std::env::var_os("BHPC_DATA_DIR"), LaunchConfig::default_data_dir()).map_err(io)?;
    Ok(ops::mods_dirs(&paths, default))
}

fn prefs() -> Prefs {
    ops::paths().ok().and_then(|p| Prefs::load(&p.prefs()).ok()).unwrap_or_default()
}

/// The public index: `BHPC_MODS_INDEX` (a file or link, for development and tests) or the published one.
fn index_url() -> String {
    std::env::var("BHPC_MODS_INDEX").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| index::INDEX_URL.to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> MRes<T> + Send + 'static) -> MRes<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(io)?
}

#[tauri::command]
pub async fn mods_overview() -> MRes<Overview> {
    blocking(|| {
        let d = dirs()?;
        let idx = index::cached_index(&d.index_cache());
        Ok(mods::overview(&d.mods, idx.as_ref(), prefs().mods_block_native))
    })
    .await
}

fn load(d: &ModsDirs, refresh: bool) -> MRes<(IndexFile, bool, Option<ModError>)> {
    match index::cached_index(&d.index_cache()) {
        Some(idx) if !refresh => Ok((idx, false, None)),
        _ => index::load_index(&Net, &index_url(), &d.index_cache()),
    }
}

/// The public list (`refresh`: download it again, else the copy from the last download when there is one).
#[tauri::command]
pub async fn mods_index(refresh: bool) -> MRes<Browse> {
    blocking(move || {
        let d = dirs()?;
        let (idx, offline, err) = load(&d, refresh)?;
        Ok(index::browse(&idx, &mods::installed_versions(&d.mods), offline, err))
    })
    .await
}

#[tauri::command]
pub async fn mods_plan(id: String, version: Option<String>) -> MRes<Plan> {
    blocking(move || {
        let d = dirs()?;
        let (idx, ..) = load(&d, false)?;
        index::plan_index(&idx, &mods::installed_versions(&d.mods), &id, version.as_deref())
    })
    .await
}

fn hold(pending: &Pending, c: Candidate) -> MRes<Plan> {
    let d = dirs()?;
    let idx = index::cached_index(&d.index_cache());
    let plan = install::plan_candidate(&c, idx.as_ref(), &mods::installed_versions(&d.mods))?;
    pending.0.lock().unwrap_or_else(|e| e.into_inner()).insert(c.sha256.clone(), c);
    Ok(plan)
}

/// Checks a `.bhmod`/`.zip` the player picked or dropped; the plan's `token` installs it.
#[tauri::command]
pub async fn mods_inspect_file(pending: State<'_, Pending>, path: String) -> MRes<Plan> {
    let c = blocking(move || install::from_file(Path::new(&path))).await?;
    hold(&pending, c)
}

/// Downloads and checks a package from an HTTPS link.
#[tauri::command]
pub async fn mods_inspect_link(pending: State<'_, Pending>, url: String) -> MRes<Plan> {
    let c = blocking(move || install::from_link(&Net, url.trim())).await?;
    hold(&pending, c)
}

#[derive(Deserialize)]
pub struct InstallItem {
    id: String,
    version: String,
}

#[derive(Deserialize)]
pub struct InstallRequest {
    /// Mods from the index (the plan's items that are not the file or link).
    items: Vec<InstallItem>,
    /// A checked file or link (`Plan::token`).
    token: Option<String>,
    grants: BTreeMap<String, Vec<String>>,
    enable: bool,
    native_confirmed: bool,
}

#[derive(Clone, Serialize)]
struct InstallProgress {
    /// downloading | checking | writing
    stage: &'static str,
    done: usize,
    total: usize,
}

/// Downloads (size and SHA-256 checked), then writes everything at once with the player's choices. Progress goes out
/// as `mods-install` events.
#[tauri::command]
pub async fn mods_install(app: AppHandle, pending: State<'_, Pending>, req: InstallRequest) -> MRes<Vec<String>> {
    let held = match &req.token {
        Some(t) => Some(
            pending
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(t)
                .cloned()
                .ok_or_else(|| ModError::new("expired"))?,
        ),
        None => None,
    };
    let token = req.token.clone();
    let out = blocking(move || {
        let d = dirs()?;
        let total = req.items.len();
        let mut cands: Vec<Candidate> = held.into_iter().collect();
        if total > 0 {
            let (idx, ..) = load(&d, false)?;
            for (i, it) in req.items.iter().enumerate() {
                let _ = app.emit("mods-install", InstallProgress { stage: "downloading", done: i, total });
                cands.push(install::download_index(&Net, &idx, &it.id, &it.version)?);
            }
        }
        let _ = app.emit("mods-install", InstallProgress { stage: "checking", done: total, total });
        let p = prefs();
        let ch = Choices {
            grants: req.grants,
            enable: req.enable,
            native_confirmed: req.native_confirmed,
            block_native: p.mods_block_native,
            now: mods::now(),
        };
        let _ = app.emit("mods-install", InstallProgress { stage: "writing", done: total, total });
        install::write_packages(&d.mods, &cands, &ch)
    })
    .await?;
    if let Some(t) = token {
        pending.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&t);
    }
    Ok(out)
}

#[tauri::command]
pub async fn mods_set_enabled(id: String, on: bool) -> MRes<()> {
    blocking(move || mods::set_enabled(&dirs()?.mods, &id, on)).await
}

#[tauri::command]
pub async fn mods_set_grant(id: String, perm: String, on: bool) -> MRes<()> {
    blocking(move || mods::set_grant(&dirs()?.mods, &id, &perm, on, mods::now())).await
}

#[tauri::command]
pub async fn mods_remove(id: String) -> MRes<()> {
    blocking(move || mods::remove(&dirs()?.mods, &id)).await
}

/// "Update mods by themselves" (only when the setting is on; fetches a fresh index).
#[tauri::command]
pub async fn mods_auto_update() -> MRes<AutoUpdate> {
    blocking(|| {
        if !prefs().mods_auto_update {
            return Ok(AutoUpdate { updated: vec![], failed: vec![] });
        }
        let d = dirs()?;
        let (idx, ..) = index::load_index(&Net, &index_url(), &d.index_cache())?;
        Ok(install::auto_update(&Net, &d.mods, &idx, mods::now()))
    })
    .await
}

/// The server mods cache (kept under the size limit from the settings first).
#[tauri::command]
pub async fn mods_servers() -> MRes<CacheOverview> {
    blocking(|| {
        let d = dirs()?;
        cache::enforce_limit(&d, u64::from(prefs().mods_cache_limit_mb) * 1024 * 1024)?;
        Ok(cache::list(&d))
    })
    .await
}

#[tauri::command]
pub async fn mods_clear_server(key: String) -> MRes<CacheOverview> {
    blocking(move || {
        let d = dirs()?;
        cache::clear(&d, &key)?;
        Ok(cache::list(&d))
    })
    .await
}

#[tauri::command]
pub async fn mods_clear_servers() -> MRes<CacheOverview> {
    blocking(|| {
        let d = dirs()?;
        cache::clear_all(&d)?;
        Ok(cache::list(&d))
    })
    .await
}

#[tauri::command]
pub async fn mods_dev_inspect(path: String) -> MRes<DevMod> {
    blocking(move || dev::inspect(&PathBuf::from(path))).await
}

/// Loads or reloads a folder mod in the running game: needs the in-game loader (M6).
#[tauri::command]
pub async fn mods_dev_reload(path: String) -> MRes<()> {
    dev::reload(Path::new(&path))
}
