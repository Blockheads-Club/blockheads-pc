//! The public mod list (`index.json`, schema 1, published by `Blockheads-Club/mods`): download with an offline
//! copy, the browse view, and install plans with dependencies.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::path::Path;

use bhmod_manifest::manifest::Permissions;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::{req_ok, required_permissions, MRes, ModError, API_VERSION, LOADER_VERSION};

/// Where the public mod list is published (later also `https://blockheads.club/mods/index.json`).
pub const INDEX_URL: &str = "https://github.com/Blockheads-Club/mods/releases/download/index/index.json";
/// Largest package the index allows (and the largest download accepted).
pub const MAX_PACKAGE: u64 = 32 * 1024 * 1024;
/// Largest index file accepted.
pub const MAX_INDEX: u64 = 16 * 1024 * 1024;

/// HTTPS downloads. The launcher crate has no network code: the app passes its client in, tests a fake.
pub trait Fetch {
    /// The body of `url` (always `https://`), at most `limit` bytes (more is an error).
    fn get(&self, url: &str, limit: u64) -> MRes<Vec<u8>>;
}

/// Reads `url`: `https://` through `f`, `file://` and absolute paths from disk (fixtures, `BHPC_MODS_INDEX`);
/// anything else is refused.
pub fn fetch(f: &dyn Fetch, url: &str, limit: u64) -> MRes<Vec<u8>> {
    let local = url.strip_prefix("file://").or_else(|| url.starts_with('/').then_some(url));
    if let Some(path) = local {
        return read_local(Path::new(path), limit);
    }
    if !url.starts_with("https://") {
        return Err(ModError::with("insecure", url));
    }
    f.get(url, limit).and_then(|b| check_limit(b, limit))
}

/// A local file, at most `limit` bytes.
pub fn read_local(path: &Path, limit: u64) -> MRes<Vec<u8>> {
    let mut buf = Vec::new();
    let file = std::fs::File::open(path).map_err(|e| ModError::with("download", e.to_string()))?;
    file.take(limit + 1).read_to_end(&mut buf).map_err(|e| ModError::with("download", e.to_string()))?;
    check_limit(buf, limit)
}

/// `buf` if it is at most `limit` bytes.
pub fn check_limit(buf: Vec<u8>, limit: u64) -> MRes<Vec<u8>> {
    if buf.len() as u64 > limit {
        return Err(ModError::with("tooLarge", limit.to_string()));
    }
    Ok(buf)
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct IndexFile {
    pub schema: u32,
    pub generated: String,
    #[serde(default)]
    pub api: Vec<String>,
    pub mods: Vec<IndexMod>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct IndexMod {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub owners: Vec<String>,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub versions: Vec<IndexVersion>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct IndexVersion {
    pub version: String,
    pub url: String,
    #[serde(default)]
    pub mirror: Option<String>,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sides: Vec<String>,
    #[serde(default)]
    pub client_required: bool,
    pub api: String,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub depends: BTreeMap<String, String>,
    #[serde(default)]
    pub soft_depends: BTreeMap<String, String>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub published: String,
    #[serde(default)]
    pub yanked: bool,
}

impl IndexVersion {
    /// This launcher can run it (API and loader requirements met, semver version).
    pub fn compatible(&self) -> bool {
        req_ok(&self.api, API_VERSION) && self.loader.as_deref().is_none_or(|r| req_ok(r, LOADER_VERSION))
    }
    /// Not yanked, compatible and has a client part.
    pub fn usable(&self) -> bool {
        !self.yanked && self.compatible() && self.client()
    }
    pub fn client(&self) -> bool {
        self.sides.iter().any(|s| s == "client")
    }
    pub fn semver(&self) -> Version {
        Version::parse(&self.version).unwrap_or(Version::new(0, 0, 0))
    }
}

impl IndexMod {
    /// The newest version this launcher can run.
    pub fn latest(&self) -> Option<&IndexVersion> {
        self.versions.iter().filter(|v| v.usable()).max_by_key(|v| v.semver())
    }
    /// The newest version that is not yanked (compatible or not).
    pub fn newest(&self) -> Option<&IndexVersion> {
        self.versions.iter().filter(|v| !v.yanked).max_by_key(|v| v.semver())
    }
    /// The newest usable version matching `req`.
    pub fn best(&self, req: &VersionReq) -> Option<&IndexVersion> {
        self.versions.iter().filter(|v| v.usable() && req.matches(&v.semver())).max_by_key(|v| v.semver())
    }
}

impl IndexFile {
    pub fn find(&self, id: &str) -> Option<&IndexMod> {
        self.mods.iter().find(|m| m.id == id)
    }
}

pub fn parse_index(bytes: &[u8]) -> MRes<IndexFile> {
    let idx: IndexFile = serde_json::from_slice(bytes).map_err(|e| ModError::with("index", e.to_string()))?;
    if idx.schema != 1 {
        return Err(ModError::with("index", format!("unsupported schema {}", idx.schema)));
    }
    Ok(idx)
}

/// The index from `url`, falling back to the copy in `cache` when it cannot be fetched (`true` = offline copy, with
/// the error). Without a copy the error is `offline`.
pub fn load_index(f: &dyn Fetch, url: &str, cache: &Path) -> MRes<(IndexFile, bool, Option<ModError>)> {
    match fetch(f, url, MAX_INDEX).and_then(|b| parse_index(&b).map(|i| (i, b))) {
        Ok((idx, bytes)) => {
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(cache, bytes);
            Ok((idx, false, None))
        }
        Err(e) => match cached_index(cache) {
            Some(idx) => Ok((idx, true, Some(e))),
            None => Err(ModError { code: "offline".into(), detail: e.detail.or(Some(e.code)), ..Default::default() }),
        },
    }
}

/// The cached index, if any.
pub fn cached_index(cache: &Path) -> Option<IndexFile> {
    std::fs::read(cache).ok().and_then(|b| parse_index(&b).ok())
}

// ---------------------------------------------------------------------------------------------------------------
// Browse

/// One mod of the public list as the Mod list tab shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BrowseMod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owners: Vec<String>,
    pub repo: String,
    pub tags: Vec<String>,
    /// The newest version that is not yanked.
    pub version: String,
    /// It runs on this launcher; else `compatible_version` is the newest that does.
    pub compatible: bool,
    pub compatible_version: Option<String>,
    pub sides: Vec<String>,
    pub permissions: Vec<String>,
    pub native: bool,
    pub depends: BTreeMap<String, String>,
    pub size: Option<u64>,
    pub published: String,
    /// Versions newest first: (version, published).
    pub versions: Vec<(String, String)>,
    pub installed: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Browse {
    pub generated: String,
    pub offline: bool,
    pub error: Option<ModError>,
    pub mods: Vec<BrowseMod>,
    /// Every tag on the list, sorted.
    pub tags: Vec<String>,
}

/// The list's mods (all sides; the UI filters "for players" by `sides`).
pub fn browse(idx: &IndexFile, installed: &HashMap<String, String>, offline: bool, error: Option<ModError>) -> Browse {
    let mut mods = Vec::new();
    let mut tags = std::collections::BTreeSet::new();
    for m in &idx.mods {
        let Some(v) = m.newest() else { continue };
        tags.extend(m.tags.iter().cloned());
        let mut versions: Vec<&IndexVersion> = m.versions.iter().filter(|v| !v.yanked).collect();
        versions.sort_by_key(|v| std::cmp::Reverse(v.semver()));
        mods.push(BrowseMod {
            id: m.id.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            owners: m.owners.clone(),
            repo: m.repo.clone(),
            tags: m.tags.clone(),
            version: v.version.clone(),
            compatible: v.usable(),
            compatible_version: m.latest().map(|l| l.version.clone()),
            sides: v.sides.clone(),
            permissions: required_permissions(&v.permissions),
            native: v.permissions.native,
            depends: v.depends.clone(),
            size: v.size,
            published: v.published.clone(),
            versions: versions.iter().map(|v| (v.version.clone(), v.published.clone())).collect(),
            installed: installed.get(&m.id).cloned(),
        });
    }
    Browse { generated: idx.generated.clone(), offline, error, mods, tags: tags.into_iter().collect() }
}

// ---------------------------------------------------------------------------------------------------------------
// Install plans

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub size: Option<u64>,
    pub sides: Vec<String>,
    pub permissions: Vec<String>,
    pub native: bool,
    /// Pulled in as a dependency of this mod.
    pub needed_by: Option<String>,
    /// Replaces this installed version.
    pub replaces: Option<String>,
    /// From the public list (false: the file or link the player gave and its SHA-256 is not on the list).
    pub reviewed: bool,
    /// Who published it (index owners, or the package's authors).
    pub authors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Satisfied {
    pub id: String,
    pub req: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Missing {
    pub id: String,
    pub req: String,
    pub needed_by: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Plan {
    pub items: Vec<PlanItem>,
    pub satisfied: Vec<Satisfied>,
    pub missing: Vec<Missing>,
    pub total_bytes: u64,
    /// For a file or link: the token of the checked package held by the app until the install.
    pub token: Option<String>,
}

pub(crate) fn plan_item(
    m: &IndexMod,
    v: &IndexVersion,
    needed_by: Option<String>,
    installed: &HashMap<String, String>,
) -> PlanItem {
    PlanItem {
        id: m.id.clone(),
        name: m.name.clone(),
        version: v.version.clone(),
        size: v.size,
        sides: v.sides.clone(),
        permissions: required_permissions(&v.permissions),
        native: v.permissions.native,
        needed_by,
        replaces: installed.get(&m.id).cloned(),
        reviewed: true,
        authors: m.owners.clone(),
    }
}

/// Adds the dependencies of `root` (and theirs) from the index: installed ones that match are kept, others are
/// installed or updated, unknown ones are missing.
pub(crate) fn add_deps(
    idx: Option<&IndexFile>,
    installed: &HashMap<String, String>,
    root: (&str, &BTreeMap<String, String>),
    plan: &mut Plan,
) {
    let mut queue: VecDeque<(String, BTreeMap<String, String>)> = VecDeque::new();
    queue.push_back((root.0.to_string(), root.1.clone()));
    while let Some((owner, deps)) = queue.pop_front() {
        for (id, req_text) in deps {
            if plan.items.iter().any(|i| i.id == id) || plan.satisfied.iter().any(|s| s.id == id) {
                continue;
            }
            if let Some(v) = installed.get(&id).filter(|v| req_ok(&req_text, v)) {
                plan.satisfied.push(Satisfied { id, req: req_text, version: v.clone() });
                continue;
            }
            let req = VersionReq::parse(&req_text).unwrap_or(VersionReq::STAR);
            match idx.and_then(|i| i.find(&id)).and_then(|m| m.best(&req).map(|v| (m, v))) {
                Some((m, v)) => {
                    plan.items.push(plan_item(m, v, Some(owner.clone()), installed));
                    queue.push_back((id, v.depends.clone()));
                }
                None => plan.missing.push(Missing { id, req: req_text, needed_by: owner.clone() }),
            }
        }
    }
}

pub(crate) fn finish(mut plan: Plan) -> Plan {
    plan.total_bytes = plan.items.iter().filter_map(|i| i.size).sum();
    plan
}

/// What installing `id` (`version`, else the newest usable) from the index involves. When the newest version needs
/// a newer launcher the error is `newerLoader` (`expected`: its requirement, `detail`: the newest version that works
/// here, if any).
pub fn plan_index(idx: &IndexFile, installed: &HashMap<String, String>, id: &str, version: Option<&str>) -> MRes<Plan> {
    let m = idx.find(id).ok_or_else(|| ModError::with("notListed", id))?;
    let v = match version {
        Some(want) => m.versions.iter().find(|v| v.version == want && !v.yanked),
        None => m.newest(),
    }
    .ok_or_else(|| ModError::with("notListed", id))?;
    if !v.client() {
        return Err(ModError::with("serverOnly", id));
    }
    if !v.compatible() {
        return Err(ModError {
            code: "newerLoader".into(),
            file: Some(format!("{} {}", m.name, v.version)),
            expected: Some(v.loader.clone().unwrap_or_else(|| v.api.clone())),
            got: Some(LOADER_VERSION.into()),
            detail: m.latest().map(|l| l.version.clone()),
        });
    }
    let mut plan = Plan {
        items: vec![plan_item(m, v, None, installed)],
        satisfied: vec![],
        missing: vec![],
        total_bytes: 0,
        token: None,
    };
    add_deps(Some(idx), installed, (id, &v.depends), &mut plan);
    Ok(finish(plan))
}
