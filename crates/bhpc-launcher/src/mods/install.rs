//! Installing: checking packages (file, link or index download with size and SHA-256), plans for them and writing
//! them into the mods folder with the player's choices.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::Path;

use bhmod_manifest::package::{self, CheckOptions, ZipSource};
use bhmod_manifest::Manifest;
use serde::Serialize;

use super::index::{self, add_deps, fetch, finish, Fetch, IndexFile, IndexVersion, Plan, PlanItem, MAX_PACKAGE};
use super::{
    client_dep, discover, edit_config, io_err, read_config, read_records, required_permissions, write_records,
    InstallRecord, MRes, ModError,
};

/// A checked package waiting for the install step.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub manifest: Manifest,
    pub bytes: Vec<u8>,
    pub sha256: String,
    /// `index`, `file`, `link` (or `server` for the server cache).
    pub source: String,
    /// The file name or link, for messages and the install record.
    pub origin: String,
}

/// Checks a package with the same rules as `bhmod check` and the mod hosts.
pub fn inspect(bytes: Vec<u8>, origin: &str) -> MRes<Candidate> {
    if bytes.len() as u64 > MAX_PACKAGE {
        return Err(ModError::with("tooLarge", MAX_PACKAGE.to_string()));
    }
    let len = bytes.len() as u64;
    let mut src = ZipSource::open(Cursor::new(bytes.as_slice()), len).map_err(|e| ModError {
        file: Some(origin.into()),
        ..ModError::with("invalid", format!("not a valid zip: {e}"))
    })?;
    let report = package::check(&mut src, CheckOptions::default());
    let manifest = match report.manifest {
        Some(m) if report.errors.is_empty() => m,
        _ => {
            let msgs: Vec<String> = report.errors.iter().map(|i| i.to_string()).collect();
            return Err(ModError { file: Some(origin.into()), ..ModError::with("invalid", msgs.join("; ")) });
        }
    };
    let sha256 = bhmod_manifest::pack::sha256_hex(&bytes);
    Ok(Candidate { manifest, bytes, sha256, source: String::new(), origin: origin.into() })
}

/// A `.bhmod` or `.zip` the player picked.
pub fn from_file(path: &Path) -> MRes<Candidate> {
    let name = super::file_name(path);
    let bytes = index::read_local(path, MAX_PACKAGE).map_err(|e| ModError { file: Some(name.clone()), ..e })?;
    Ok(Candidate { source: "file".into(), ..inspect(bytes, &name)? })
}

/// A package from a link the player pasted (HTTPS only).
pub fn from_link(f: &dyn Fetch, url: &str) -> MRes<Candidate> {
    if !url.starts_with("https://") {
        return Err(ModError::with("insecure", url));
    }
    let bytes = fetch(f, url, MAX_PACKAGE)?;
    Ok(Candidate { source: "link".into(), ..inspect(bytes, url)? })
}

/// The plan for a candidate: the package itself plus its client dependencies from the index.
pub fn plan_candidate(c: &Candidate, idx: Option<&IndexFile>, installed: &HashMap<String, String>) -> MRes<Plan> {
    let m = &c.manifest;
    if !m.sides.client {
        return Err(ModError { file: Some(c.origin.clone()), ..ModError::with("serverOnly", &m.id) });
    }
    let reviewed = idx
        .and_then(|i| i.find(&m.id))
        .is_some_and(|x| x.versions.iter().any(|v| v.sha256.eq_ignore_ascii_case(&c.sha256)));
    let mut sides = Vec::new();
    if m.sides.server {
        sides.push("server".to_string());
    }
    sides.push("client".to_string());
    let item = PlanItem {
        id: m.id.clone(),
        name: m.name.get("en").or_else(|| m.name.values().next()).cloned().unwrap_or_else(|| m.id.clone()),
        version: m.version.clone(),
        size: Some(c.bytes.len() as u64),
        sides,
        permissions: required_permissions(&m.permissions),
        native: m.permissions.native,
        needed_by: None,
        replaces: installed.get(&m.id).cloned(),
        reviewed,
        authors: m.authors.clone(),
    };
    let deps: BTreeMap<String, String> = m
        .dependencies
        .iter()
        .filter(|(_, d)| client_dep(d))
        .map(|(k, d)| (k.clone(), d.version().to_string()))
        .collect();
    let mut plan =
        Plan { items: vec![item], satisfied: vec![], missing: vec![], total_bytes: 0, token: Some(c.sha256.clone()) };
    add_deps(idx, installed, (&m.id, &deps), &mut plan);
    Ok(finish(plan))
}

/// Downloads one index version of `id` and checks its size, SHA-256 and manifest (`mirror` is tried when `url`
/// fails).
pub fn download(f: &dyn Fetch, id: &str, v: &IndexVersion) -> MRes<Candidate> {
    let file = format!("{id}-{}.bhmod", v.version);
    let limit = v.size.unwrap_or(MAX_PACKAGE).min(MAX_PACKAGE);
    let bytes = match fetch(f, &v.url, limit) {
        Ok(b) => b,
        Err(first) => match &v.mirror {
            Some(m) => fetch(f, m, limit).map_err(|_| first)?,
            None => return Err(first),
        },
    };
    if let Some(size) = v.size.filter(|s| *s != bytes.len() as u64) {
        return Err(ModError {
            file: Some(file),
            expected: Some(size.to_string()),
            got: Some(bytes.len().to_string()),
            ..ModError::new("size")
        });
    }
    let got = bhmod_manifest::pack::sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(&v.sha256) {
        return Err(ModError {
            file: Some(file),
            expected: Some(v.sha256.clone()),
            got: Some(got),
            ..ModError::new("checksum")
        });
    }
    let c = inspect(bytes, &file)?;
    if c.manifest.id != id || c.manifest.version != v.version {
        return Err(ModError {
            file: Some(file),
            expected: Some(format!("{id} {}", v.version)),
            got: Some(format!("{} {}", c.manifest.id, c.manifest.version)),
            ..ModError::new("mismatch")
        });
    }
    Ok(Candidate { source: "index".into(), origin: v.url.clone(), ..c })
}

/// Downloads `id` at `version` from the index.
pub fn download_index(f: &dyn Fetch, idx: &IndexFile, id: &str, version: &str) -> MRes<Candidate> {
    let v = idx
        .find(id)
        .and_then(|m| m.versions.iter().find(|v| v.version == version && !v.yanked))
        .ok_or_else(|| ModError::with("notListed", id))?;
    download(f, id, v)
}

/// The player's choices for an install.
#[derive(Debug, Clone, Default)]
pub struct Choices {
    /// Granted permissions per mod id (names the mod does not ask for are dropped).
    pub grants: BTreeMap<String, Vec<String>>,
    /// Turn the mods on.
    pub enable: bool,
    /// The player ticked both native boxes (needed for any native package).
    pub native_confirmed: bool,
    /// "Never allow native mods" is on: native packages are refused.
    pub block_native: bool,
    /// Unix seconds.
    pub now: u64,
}

/// Writes checked packages into the mods folder (replacing other versions of the same mods) and records the choices.
/// Nothing is written when any package is refused.
pub fn write_packages(dir: &Path, cands: &[Candidate], ch: &Choices) -> MRes<Vec<String>> {
    for c in cands {
        let m = &c.manifest;
        let file = Some(c.origin.clone());
        if !m.sides.client {
            return Err(ModError { file, ..ModError::with("serverOnly", &m.id) });
        }
        if m.permissions.native && ch.block_native {
            return Err(ModError { file, ..ModError::with("nativeBlocked", &m.id) });
        }
        if m.permissions.native && !ch.native_confirmed {
            return Err(ModError { file, ..ModError::with("nativeConfirm", &m.id) });
        }
    }
    read_config(dir)?;
    std::fs::create_dir_all(dir).map_err(io_err)?;
    let (old, _) = discover(dir);
    let mut written = Vec::new();
    for c in cands {
        let m = &c.manifest;
        let name = format!("{}-{}.bhmod", m.id, m.version);
        let part = dir.join(format!(".{name}.part"));
        std::fs::write(&part, &c.bytes).map_err(io_err)?;
        for p in old.iter().filter(|p| p.manifest.id == m.id && p.path != dir.join(&name)) {
            std::fs::remove_file(&p.path).map_err(io_err)?;
        }
        std::fs::rename(&part, dir.join(&name)).map_err(io_err)?;
        written.push(m.id.clone());
    }
    edit_config(dir, |cfg| {
        for c in cands {
            let m = &c.manifest;
            let asked = required_permissions(&m.permissions);
            let e = cfg.entry(m.id.clone()).or_default();
            e.enabled = ch.enable;
            let mut g: Vec<String> = ch
                .grants
                .get(&m.id)
                .into_iter()
                .flatten()
                .filter(|p| asked.contains(p) && *p != "native")
                .cloned()
                .collect();
            if m.permissions.native {
                g.push("native".into());
            }
            g.sort();
            g.dedup();
            e.grant = g;
        }
    })?;
    let mut recs = read_records(dir);
    for c in cands {
        let native_allowed = c.manifest.permissions.native.then_some(ch.now);
        recs.insert(
            c.manifest.id.clone(),
            InstallRecord { source: c.source.clone(), origin: c.origin.clone(), installed: ch.now, native_allowed },
        );
    }
    write_records(dir, &recs)?;
    Ok(written)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutoUpdate {
    pub updated: Vec<(String, String)>,
    pub failed: Vec<(String, ModError)>,
}

/// "Update mods by themselves": installs every update from the index that asks for nothing new (native mods and
/// mods whose update needs new permissions wait for the player). The mod keeps its on/off state and grants.
pub fn auto_update(f: &dyn Fetch, dir: &Path, idx: &IndexFile, now: u64) -> AutoUpdate {
    let ov = super::overview(dir, Some(idx), false);
    let cfg = read_config(dir).unwrap_or_default();
    let mut out = AutoUpdate { updated: Vec::new(), failed: Vec::new() };
    for m in ov.mods.iter().filter(|m| !m.native && !m.update_asks_more) {
        let Some(version) = &m.update else { continue };
        let c = cfg.get(&m.id).cloned().unwrap_or_default();
        // A package that turns out native is refused by write_packages (no native confirmation here).
        let res = download_index(f, idx, &m.id, version).and_then(|cand| {
            let ch = Choices {
                grants: BTreeMap::from([(m.id.clone(), c.grant.clone())]),
                enable: c.enabled,
                now,
                ..Choices::default()
            };
            write_packages(dir, &[cand], &ch)
        });
        match res {
            Ok(_) => out.updated.push((m.id.clone(), version.clone())),
            Err(e) => out.failed.push((m.id.clone(), e)),
        }
    }
    out
}

/// The installed versions as the index module wants them.
pub fn installed(dir: &Path) -> HashMap<String, String> {
    super::installed_versions(dir)
}

pub use index::plan_index;
