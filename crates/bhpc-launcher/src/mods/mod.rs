//! Client mods: what is installed in `<data dir>/mods`, the player's choices (on/off, permission grants in
//! `mods.toml`, the same format the mod hosts read), the public mod index, installs with size and SHA-256 checks
//! and the per-server cache of mods that servers send.
//!
//! The in-game mod loader (`bhpc-run`, crate `bhpc-mods`) runs the mods; this module manages the files, so a mod
//! that is ready to run reports `ready` here and the running game's report ([`live`]) turns that into `running`,
//! `budget` or `failed`.
//!
//! Layout below the data folder:
//!
//! ```text
//! mods/                     the player's client mods
//!   <id>-<version>.bhmod
//!   mods.toml               on/off and granted permissions (bhmod-core format)
//!   .installed.toml         where each mod came from and when (launcher only)
//! mods-cache/
//!   index.json              last copy of the public mod list (offline fallback)
//!   blobs/<sha256>.bhmod    mods servers sent, stored once even when several servers use them
//!   servers/<key>.toml      one per server: name, address, last played, the mods it uses
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use bhmod_manifest::manifest::{DepSide, Dependency, Permissions, PlayerPermission, WorldAccess};
use bhmod_manifest::Manifest;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

pub mod cache;
pub mod dev;
pub mod index;
pub mod install;
pub mod live;

pub use index::{Fetch, IndexFile};
pub use install::Candidate;

/// The `blockheads:api` version the client mod API will implement (the mod-sdk's).
pub const API_VERSION: &str = bhmod_manifest::API_VERSION;
/// The client mod loader version a package's `loader` requirement is checked against.
pub const LOADER_VERSION: &str = "0.1.0";
/// The settings file inside the mods folder (bhmod-core's `mods.toml`).
pub const CONFIG_FILE: &str = "mods.toml";
/// Install records inside the mods folder (hidden: mod hosts skip dot files).
pub const META_FILE: &str = ".installed.toml";

/// An error the UI shows with its own text: `code` picks the message, the rest fills it in.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ModError {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub got: Option<String>,
}

impl ModError {
    pub fn new(code: &str) -> Self {
        ModError { code: code.into(), ..Default::default() }
    }
    pub fn with(code: &str, detail: impl Into<String>) -> Self {
        ModError { code: code.into(), detail: Some(detail.into()), ..Default::default() }
    }
}

impl std::fmt::Display for ModError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{}: {d}", self.code),
            None => f.write_str(&self.code),
        }
    }
}

pub(crate) fn io_err(e: std::io::Error) -> ModError {
    ModError::with("io", e.to_string())
}

pub type MRes<T> = Result<T, ModError>;

/// Where the mod files live, below one data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModsDirs {
    /// The player's client mods.
    pub mods: PathBuf,
    /// Index copy and server mods.
    pub cache: PathBuf,
}

impl ModsDirs {
    pub fn new(data_dir: &Path) -> ModsDirs {
        ModsDirs { mods: data_dir.join("mods"), cache: data_dir.join("mods-cache") }
    }
    /// The last copy of the public index.
    pub fn index_cache(&self) -> PathBuf {
        self.cache.join("index.json")
    }
    /// Server-mod packages by SHA-256 (`<sha256>.bhmod`, partial downloads as `.part`).
    pub fn blobs(&self) -> PathBuf {
        self.cache.join("blobs")
    }
    pub(crate) fn servers(&self) -> PathBuf {
        self.cache.join("servers")
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Permissions

fn player_permission_name(p: PlayerPermission) -> &'static str {
    match p {
        PlayerPermission::Chat => "chat",
        PlayerPermission::Teleport => "teleport",
        PlayerPermission::Kick => "kick",
        PlayerPermission::Ban => "ban",
        PlayerPermission::Inventory => "inventory",
        PlayerPermission::Health => "health",
    }
}

/// The permission names a mod asks for (bhmod-core's names: `world:read`, `players:chat`, `net-http:<host>`,
/// `channels`, `native`).
pub fn required_permissions(p: &Permissions) -> Vec<String> {
    let mut out = Vec::new();
    match p.world {
        WorldAccess::None => {}
        WorldAccess::Read => out.push("world:read".to_string()),
        WorldAccess::Write => out.push("world:write".to_string()),
    }
    out.extend(p.players.iter().map(|x| format!("players:{}", player_permission_name(*x))));
    out.extend(p.net_http.iter().map(|h| format!("net-http:{h}")));
    if p.channels {
        out.push("channels".to_string());
    }
    if p.native {
        out.push("native".to_string());
    }
    out
}

/// Required permissions that `grant` does not cover (`world:write` covers `world:read`).
pub fn missing_permissions(required: &[String], grant: &[String]) -> Vec<String> {
    let has = |p: &str| grant.iter().any(|g| g == p) || (p == "world:read" && grant.iter().any(|g| g == "world:write"));
    required.iter().filter(|p| !has(p)).cloned().collect()
}

// ---------------------------------------------------------------------------------------------------------------
// mods.toml and the install records

/// One `mods.toml` entry (bhmod-core's `ModConfig`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModConfig {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grant: Vec<String>,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Default for ModConfig {
    fn default() -> Self {
        ModConfig { enabled: true, grant: Vec::new() }
    }
}

pub type ModsConfig = BTreeMap<String, ModConfig>;

/// Reads `mods.toml`; a missing file is empty, an invalid one is an error (it is never overwritten).
pub fn read_config(dir: &Path) -> MRes<ModsConfig> {
    match std::fs::read_to_string(dir.join(CONFIG_FILE)) {
        Ok(t) => toml::from_str(&t).map_err(|e| ModError::with("config", e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ModsConfig::new()),
        Err(e) => Err(io_err(e)),
    }
}

/// Changes `mods.toml` with `f`; entries back at the defaults are dropped.
pub fn edit_config(dir: &Path, f: impl FnOnce(&mut ModsConfig)) -> MRes<()> {
    let mut cfg = read_config(dir)?;
    f(&mut cfg);
    cfg.retain(|_, c| *c != ModConfig::default());
    let body = toml::to_string(&cfg).expect("mods.toml always serialises");
    std::fs::create_dir_all(dir).map_err(io_err)?;
    let text = format!(
        "# Client mod settings: on/off and granted permissions. Blockheads PC keeps this file up to date.\n\n{body}"
    );
    std::fs::write(dir.join(CONFIG_FILE), text).map_err(io_err)
}

/// Where an installed mod came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallRecord {
    /// `index` (the public list), `file`, `link` or `dev`.
    pub source: String,
    /// The file name or link it was installed from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// Unix seconds.
    pub installed: u64,
    /// When the player allowed native code (unix seconds), for native mods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_allowed: Option<u64>,
}

pub type Records = BTreeMap<String, InstallRecord>;

/// The install records; a missing or broken file is empty (they are only shown, never trusted).
pub fn read_records(dir: &Path) -> Records {
    std::fs::read_to_string(dir.join(META_FILE)).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_records(dir: &Path, r: &Records) -> MRes<()> {
    std::fs::create_dir_all(dir).map_err(io_err)?;
    std::fs::write(dir.join(META_FILE), toml::to_string(r).expect("records always serialise")).map_err(io_err)
}

// ---------------------------------------------------------------------------------------------------------------
// Installed packages

/// A checked `.bhmod` in the mods folder.
#[derive(Debug, Clone)]
pub struct Package {
    pub manifest: Manifest,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub message: String,
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Every `.bhmod` in `dir` (sorted by name), checked with the package rules; broken ones and second copies of an id
/// are problems.
pub fn discover(dir: &Path) -> (Vec<Package>, Vec<Problem>) {
    let mut pkgs: Vec<Package> = Vec::new();
    let mut problems = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return (pkgs, problems) };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = file_name(&path);
        if name.starts_with('.') || !name.ends_with(".bhmod") || !path.is_file() {
            continue;
        }
        let checked = std::fs::read(&path).map_err(io_err).and_then(|b| install::inspect(b, &name));
        match checked {
            Ok(c) if pkgs.iter().any(|p| p.manifest.id == c.manifest.id) => problems
                .push(Problem { file: name, message: format!("another file already has the mod {}", c.manifest.id) }),
            Ok(c) => pkgs.push(Package { size: c.bytes.len() as u64, manifest: c.manifest, path, sha256: c.sha256 }),
            Err(e) => problems.push(Problem { file: name, message: e.to_string() }),
        }
    }
    (pkgs, problems)
}

/// Installed mods: id -> version.
pub fn installed_versions(dir: &Path) -> HashMap<String, String> {
    discover(dir).0.into_iter().map(|p| (p.manifest.id, p.manifest.version)).collect()
}

/// A dependency needed on the client.
pub(crate) fn client_dep(d: &Dependency) -> bool {
    match d {
        Dependency::Req(_) => true,
        Dependency::Detailed { side, .. } => *side != DepSide::Server,
    }
}

/// `req` (a semver requirement) accepts `version`.
/// True when the host serves an API version the requirement accepts: it serves every version in
/// `SUPPORTED_API_VERSIONS` (0.x minors only add), not only the newest.
pub(crate) fn api_ok(req: &str) -> bool {
    bhmod_manifest::SUPPORTED_API_VERSIONS.iter().any(|v| req_ok(req, v))
}

pub(crate) fn req_ok(req: &str, version: &str) -> bool {
    match (VersionReq::parse(req), Version::parse(version)) {
        (Ok(r), Ok(v)) => r.matches(&v),
        _ => false,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepInfo {
    pub id: String,
    pub req: String,
    /// Installed version, if any.
    pub installed: Option<String>,
    /// Installed, on and the version matches.
    pub ok: bool,
    /// A soft dependency (used when present).
    pub soft: bool,
    /// A version on the public list that would satisfy it.
    pub available: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Sides {
    pub server: bool,
    pub client: bool,
}

/// One installed mod as the Mods screen shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModSummary {
    pub id: String,
    pub name: BTreeMap<String, String>,
    pub description: BTreeMap<String, String>,
    pub version: String,
    pub authors: Vec<String>,
    pub license: String,
    pub homepage: Option<String>,
    pub sides: Sides,
    /// Client runtime (`lua`, `wasm`, `jvm`).
    pub runtime: Option<String>,
    /// The `blockheads:api` requirement (packages that this launcher's API does not meet are refused as invalid).
    pub api: String,
    /// Unsandboxed native code.
    pub native: bool,
    /// Permission names the mod asks for, what is granted and what is missing.
    pub permissions: Vec<String>,
    pub granted: Vec<String>,
    pub missing: Vec<String>,
    /// On in `mods.toml`.
    pub enabled: bool,
    /// off | ready | permissions | dependency | failed | blocked (and, from the loader in M6, running | budget).
    pub status: String,
    /// For `failed`: `loader` or `server-only`; for `blocked`: `native`.
    pub reason: Option<String>,
    /// For `dependency`: "id req" of the first missing one; for `permissions`: the missing names.
    pub detail: Option<String>,
    pub dependencies: Vec<DepInfo>,
    pub file: String,
    pub sha256: String,
    pub size: u64,
    /// The package is a version from the public list (its SHA-256 matches).
    pub reviewed: bool,
    /// A newer usable version on the public list.
    pub update: Option<String>,
    /// That version asks for permissions this one does not.
    pub update_asks_more: bool,
    /// `index`, `file`, `link`, `dev` or empty when unknown.
    pub source: String,
    pub origin: String,
    pub installed_at: u64,
    pub native_allowed_at: Option<u64>,
    /// The running game's report for this mod (see [`live`]).
    pub live: Option<live::LiveMod>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Overview {
    pub dir: String,
    pub size: u64,
    pub mods: Vec<ModSummary>,
    pub problems: Vec<Problem>,
    /// The folder or `mods.toml` cannot be read.
    pub error: Option<ModError>,
    /// Mods needing attention (failed, blocked, dependency, permissions): the count on the Home button.
    pub attention: usize,
    /// Mods with an update on the public list.
    pub updates: usize,
    /// The in-game loader runs client mods (Blockheads PC 0.2 and later).
    pub loader_ready: bool,
}

fn runtime_name(m: &Manifest) -> Option<String> {
    let e = m.entry.client.as_ref()?;
    Some(format!("{:?}", e.runtime).to_lowercase())
}

/// Statuses that need the player's attention.
pub const ATTENTION: [&str; 5] = ["failed", "blocked", "dependency", "permissions", "budget"];

/// Every installed mod with its status. `index` gives the reviewed and update flags; `block_native` is the
/// "never allow native mods" setting.
pub fn overview(dir: &Path, idx: Option<&IndexFile>, block_native: bool) -> Overview {
    let mut ov = Overview {
        dir: dir.to_string_lossy().into_owned(),
        size: 0,
        mods: Vec::new(),
        problems: Vec::new(),
        error: None,
        attention: 0,
        updates: 0,
        loader_ready: true,
    };
    if let Err(e) = std::fs::read_dir(dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            ov.error = Some(ModError { file: Some(ov.dir.clone()), ..io_err(e) });
        }
        return ov;
    }
    let cfg = read_config(dir).unwrap_or_else(|e| {
        ov.error = Some(e);
        ModsConfig::new()
    });
    let records = read_records(dir);
    let (pkgs, problems) = discover(dir);
    ov.problems = problems;
    let installed: HashMap<&str, (&str, bool)> = pkgs
        .iter()
        .map(|p| {
            (p.manifest.id.as_str(), (p.manifest.version.as_str(), cfg.get(&p.manifest.id).is_none_or(|c| c.enabled)))
        })
        .collect();
    for p in &pkgs {
        let m = &p.manifest;
        let c = cfg.get(&m.id).cloned().unwrap_or_default();
        let required = required_permissions(&m.permissions);
        let missing = missing_permissions(&required, &c.grant);
        let mut deps = Vec::new();
        for (soft, map) in [(false, &m.dependencies), (true, &m.soft_dependencies)] {
            for (id, d) in map.iter().filter(|(_, d)| client_dep(d)) {
                let inst = installed.get(id.as_str());
                let available = idx
                    .and_then(|i| i.find(id))
                    .and_then(|im| VersionReq::parse(d.version()).ok().and_then(|r| im.best(&r)))
                    .map(|v| v.version.clone());
                deps.push(DepInfo {
                    id: id.clone(),
                    req: d.version().to_string(),
                    installed: inst.map(|(v, _)| v.to_string()),
                    ok: inst.is_some_and(|(v, on)| *on && req_ok(d.version(), v)),
                    soft,
                    available,
                });
            }
        }
        let loader_ok = m.loader.as_deref().is_none_or(|r| req_ok(r, LOADER_VERSION));
        let (status, reason, detail) = if !c.enabled {
            ("off", None, None)
        } else if m.permissions.native && block_native {
            ("blocked", Some("native"), None)
        } else if !m.sides.client {
            ("failed", Some("server-only"), None)
        } else if !loader_ok {
            ("failed", Some("loader"), m.loader.clone())
        } else if !missing.is_empty() {
            ("permissions", None, Some(missing.join(", ")))
        } else if let Some(d) = deps.iter().find(|d| !d.ok && !d.soft) {
            ("dependency", None, Some(format!("{} {}", d.id, d.req)))
        } else {
            ("ready", None, None)
        };
        let index_mod = idx.and_then(|i| i.find(&m.id));
        let reviewed = index_mod.is_some_and(|im| im.versions.iter().any(|v| v.sha256.eq_ignore_ascii_case(&p.sha256)));
        let newer = index_mod.and_then(|im| im.latest()).filter(|v| v.semver() > m.semver());
        let update_asks_more = newer.is_some_and(|v| {
            let now: BTreeSet<String> = required.iter().cloned().collect();
            required_permissions(&v.permissions).iter().any(|x| !now.contains(x))
        });
        let rec = records.get(&m.id).cloned().unwrap_or_default();
        if ATTENTION.contains(&status) {
            ov.attention += 1;
        }
        if newer.is_some() {
            ov.updates += 1;
        }
        ov.size += p.size;
        ov.mods.push(ModSummary {
            id: m.id.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            version: m.version.clone(),
            authors: m.authors.clone(),
            license: m.license.clone(),
            homepage: m.homepage.clone(),
            sides: Sides { server: m.sides.server, client: m.sides.client },
            runtime: runtime_name(m),
            api: m.api.clone(),
            native: m.permissions.native,
            permissions: required,
            granted: c.grant.clone(),
            missing,
            enabled: c.enabled,
            status: status.into(),
            reason: reason.map(String::from),
            detail,
            dependencies: deps,
            file: file_name(&p.path),
            sha256: p.sha256.clone(),
            size: p.size,
            reviewed,
            update: newer.map(|v| v.version.clone()),
            update_asks_more,
            source: rec.source,
            origin: rec.origin,
            installed_at: rec.installed,
            native_allowed_at: rec.native_allowed,
            live: None,
        });
    }
    ov
}

/// The installed package of `id`.
pub fn find_package(dir: &Path, id: &str) -> MRes<Package> {
    discover(dir).0.into_iter().find(|p| p.manifest.id == id).ok_or_else(|| ModError::with("missing", id))
}

pub fn set_enabled(dir: &Path, id: &str, on: bool) -> MRes<()> {
    find_package(dir, id)?;
    edit_config(dir, |c| c.entry(id.into()).or_default().enabled = on)
}

/// Grants or revokes one permission the mod asks for. Revoking `native` also turns the mod off; granting it is only
/// done by an install with the native confirmation ([`install::write_packages`]).
pub fn set_grant(dir: &Path, id: &str, perm: &str, on: bool, now: u64) -> MRes<()> {
    let p = find_package(dir, id)?;
    if on && (perm == "native" || !required_permissions(&p.manifest.permissions).iter().any(|r| r == perm)) {
        return Err(ModError::with("permission", perm));
    }
    edit_config(dir, |c| {
        let e = c.entry(id.into()).or_default();
        e.grant.retain(|x| x != perm);
        if on {
            e.grant.push(perm.into());
            e.grant.sort();
        } else if perm == "native" {
            e.enabled = false;
        }
    })?;
    if perm == "native" {
        let mut r = read_records(dir);
        r.entry(id.into()).or_insert_with(|| InstallRecord { installed: now, ..Default::default() }).native_allowed =
            None;
        write_records(dir, &r)?;
    }
    Ok(())
}

/// Deletes the installed package of `id`, its `mods.toml` entry and its install record.
pub fn remove(dir: &Path, id: &str) -> MRes<()> {
    let p = find_package(dir, id)?;
    std::fs::remove_file(&p.path).map_err(io_err)?;
    edit_config(dir, |c| {
        c.remove(id);
    })?;
    let mut r = read_records(dir);
    if r.remove(id).is_some() {
        write_records(dir, &r)?;
    }
    Ok(())
}

/// Unix seconds now.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests;
