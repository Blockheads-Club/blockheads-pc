//! Mods that servers send (the join flow over the 0xBC protocol, milestone M5/M6): stored once per SHA-256 in
//! `mods-cache/blobs`, with one record per server in `mods-cache/servers/<key>.toml`. They only ever run on the
//! server that sent them. The join flow will call [`record_join`] and [`cached_blob`]; the launcher lists and
//! clears the cache and keeps it under the size limit from the settings.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{io_err, Candidate, MRes, ModError, ModsDirs};

/// One mod a server uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedMod {
    pub id: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub native: bool,
}

/// `servers/<key>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRecord {
    pub name: String,
    /// `host:port`.
    pub address: String,
    /// Unix seconds.
    pub last_played: u64,
    #[serde(default)]
    pub mods: Vec<CachedMod>,
}

/// A cached mod as the Server mods tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CachedModView {
    #[serde(flatten)]
    pub m: CachedMod,
    /// Names of the other servers that use the same file.
    pub shared_with: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerCache {
    pub key: String,
    pub name: String,
    pub address: String,
    pub last_played: u64,
    pub size: u64,
    pub mods: Vec<CachedModView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheOverview {
    /// Most recently played first.
    pub servers: Vec<ServerCache>,
    /// Bytes on disk (shared files counted once).
    pub total: u64,
}

/// A file-name-safe key for a server address (`Mira.example.net:15151` -> `mira.example.net_15151`).
pub fn server_key(address: &str) -> String {
    let k: String = address
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    let k = k.trim_start_matches('.').to_string();
    if k.is_empty() {
        "_".into()
    } else {
        k
    }
}

fn blob_path(dirs: &ModsDirs, sha256: &str) -> PathBuf {
    dirs.blobs().join(format!("{}.bhmod", sha256.to_ascii_lowercase()))
}

/// The stored file with this SHA-256, if any (the join flow skips its download).
pub fn cached_blob(dirs: &ModsDirs, sha256: &str) -> Option<PathBuf> {
    let p = blob_path(dirs, sha256);
    (sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()) && p.is_file()).then_some(p)
}

fn read_servers(dirs: &ModsDirs) -> Vec<(String, ServerRecord)> {
    let Ok(rd) = std::fs::read_dir(dirs.servers()) else { return Vec::new() };
    let mut out: Vec<(String, ServerRecord)> = rd
        .flatten()
        .filter_map(|e| {
            let name = super::file_name(&e.path());
            let key = name.strip_suffix(".toml")?.to_string();
            let rec = std::fs::read_to_string(e.path()).ok().and_then(|t| toml::from_str(&t).ok())?;
            Some((key, rec))
        })
        .collect();
    out.sort_by(|a, b| b.1.last_played.cmp(&a.1.last_played).then(a.0.cmp(&b.0)));
    out
}

/// The record of the server at `address` (its last join), if any.
pub fn server_record(dirs: &ModsDirs, address: &str) -> Option<ServerRecord> {
    let text = std::fs::read_to_string(dirs.servers().join(format!("{}.toml", server_key(address)))).ok()?;
    toml::from_str(&text).ok()
}

/// After a join: stores the server's checked packages (once per SHA-256) and its record.
pub fn record_join(dirs: &ModsDirs, name: &str, address: &str, cands: &[Candidate], now: u64) -> MRes<String> {
    std::fs::create_dir_all(dirs.blobs()).map_err(io_err)?;
    std::fs::create_dir_all(dirs.servers()).map_err(io_err)?;
    let mut mods = Vec::new();
    for c in cands {
        let p = blob_path(dirs, &c.sha256);
        if !p.is_file() {
            let part = p.with_extension("part");
            std::fs::write(&part, &c.bytes).map_err(io_err)?;
            std::fs::rename(&part, &p).map_err(io_err)?;
        }
        let m = &c.manifest;
        mods.push(CachedMod {
            id: m.id.clone(),
            name: m.name.get("en").or_else(|| m.name.values().next()).cloned().unwrap_or_else(|| m.id.clone()),
            version: m.version.clone(),
            sha256: c.sha256.clone(),
            size: c.bytes.len() as u64,
            native: m.permissions.native,
        });
    }
    let key = server_key(address);
    let rec = ServerRecord { name: name.into(), address: address.into(), last_played: now, mods };
    let text = toml::to_string(&rec).expect("server record always serialises");
    std::fs::write(dirs.servers().join(format!("{key}.toml")), text).map_err(io_err)?;
    Ok(key)
}

/// Every server with cached mods.
pub fn list(dirs: &ModsDirs) -> CacheOverview {
    let servers = read_servers(dirs);
    let mut users: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (_, r) in &servers {
        for m in &r.mods {
            users.entry(m.sha256.as_str()).or_default().push(r.name.as_str());
        }
    }
    let views = servers
        .iter()
        .map(|(key, r)| ServerCache {
            key: key.clone(),
            name: r.name.clone(),
            address: r.address.clone(),
            last_played: r.last_played,
            size: r.mods.iter().map(|m| m.size).sum(),
            mods: r
                .mods
                .iter()
                .map(|m| CachedModView {
                    m: m.clone(),
                    shared_with: users[m.sha256.as_str()]
                        .iter()
                        .filter(|n| **n != r.name)
                        .map(|n| n.to_string())
                        .collect(),
                })
                .collect(),
        })
        .collect();
    CacheOverview { servers: views, total: crate::datadir::dir_size(&dirs.blobs()) }
}

/// Deletes stored files no server uses any more.
pub fn gc(dirs: &ModsDirs) -> MRes<()> {
    let used: BTreeSet<String> = read_servers(dirs)
        .into_iter()
        .flat_map(|(_, r)| r.mods.into_iter().map(|m| format!("{}.bhmod", m.sha256.to_ascii_lowercase())))
        .collect();
    let Ok(rd) = std::fs::read_dir(dirs.blobs()) else { return Ok(()) };
    for e in rd.flatten() {
        if !used.contains(&super::file_name(&e.path())) {
            std::fs::remove_file(e.path()).map_err(io_err)?;
        }
    }
    Ok(())
}

/// Forgets one server's mods (files other servers use stay).
pub fn clear(dirs: &ModsDirs, key: &str) -> MRes<()> {
    let p = dirs.servers().join(format!("{key}.toml"));
    if key.is_empty() || key.contains(['/', '\\']) || key.starts_with('.') || !p.is_file() {
        return Err(ModError::with("missing", key));
    }
    std::fs::remove_file(p).map_err(io_err)?;
    gc(dirs)
}

/// Forgets every server's mods.
pub fn clear_all(dirs: &ModsDirs) -> MRes<()> {
    for d in [dirs.servers(), dirs.blobs()] {
        match std::fs::remove_dir_all(&d) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_err(e)),
            _ => {}
        }
    }
    Ok(())
}

/// Keeps the cache at most `limit` bytes (0 = no limit) by clearing the servers played longest ago. Returns the
/// cleared keys.
pub fn enforce_limit(dirs: &ModsDirs, limit: u64) -> MRes<Vec<String>> {
    let mut cleared = Vec::new();
    if limit == 0 {
        return Ok(cleared);
    }
    for (key, _) in read_servers(dirs).into_iter().rev() {
        if crate::datadir::dir_size(&dirs.blobs()) <= limit {
            break;
        }
        clear(dirs, &key)?;
        cleared.push(key);
    }
    Ok(cleared)
}
