//! The mods of one game session in one folder that [`bhmod_core::ModHost`] loads: the player's enabled client mods
//! (`<data>/mods`, with their `mods.toml` grants), developer folder mods, and the mods a server sent (accepting the
//! join prompt grants what they ask for). A server mod wins over an installed or folder mod with the same id: the
//! server needs its own version.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use bhpc_launcher::mods::{self, ModConfig};

use crate::join::Accepted;

/// What goes into a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The player's mods folder (`None`: no client mods of their own).
    pub own: Option<PathBuf>,
    /// Developer-mode folder mods (their permissions are granted: the developer loads their own code).
    pub dev: Vec<PathBuf>,
    /// Mods from the server.
    pub server: Vec<Accepted>,
}

/// One mod placed in the session, for the Mods panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub id: String,
    /// `mine`, `dev` or `server`.
    pub origin: &'static str,
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

/// Empties `dir` and fills it for `plan`. Returns the mods placed, in folder order, and problems (skipped mods).
pub fn build(dir: &Path, plan: &Plan) -> io::Result<(Vec<Placed>, Vec<String>)> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    let mut config: BTreeMap<String, ModConfig> = BTreeMap::new();
    let mut placed = Vec::new();
    let mut problems = Vec::new();
    for (n, a) in plan.server.iter().enumerate() {
        std::fs::copy(&a.path, dir.join(format!("s{n:02}-{}.bhmod", a.id)))?;
        config.insert(a.id.clone(), ModConfig { enabled: true, grant: a.grant.clone() });
        placed.push(Placed { id: a.id.clone(), origin: "server" });
    }
    for (n, d) in plan.dev.iter().enumerate() {
        match mods::dev::inspect(d) {
            Ok(m) if m.errors.is_empty() => {
                let id = m.id.expect("a folder without errors has an id");
                if config.contains_key(&id) {
                    problems.push(format!("{id} (folder {}): the server's version is used", d.display()));
                    continue;
                }
                copy_dir(d, &dir.join(format!("d{n:02}-{id}")))?;
                config.insert(id.clone(), ModConfig { enabled: true, grant: m.permissions });
                placed.push(Placed { id, origin: "dev" });
            }
            Ok(m) => problems.push(format!("folder {}: {}", d.display(), m.errors.join("; "))),
            Err(e) => problems.push(format!("folder {}: {e}", d.display())),
        }
    }
    if let Some(own) = &plan.own {
        let cfg = mods::read_config(own).unwrap_or_else(|e| {
            problems.push(format!("{}: {e}", own.join(mods::CONFIG_FILE).display()));
            mods::ModsConfig::new()
        });
        let (pkgs, bad) = mods::discover(own);
        problems.extend(bad.into_iter().map(|p| format!("{}: {}", p.file, p.message)));
        for p in pkgs {
            let id = p.manifest.id.clone();
            let c = cfg.get(&id).cloned().unwrap_or_default();
            if !c.enabled {
                continue;
            }
            if config.contains_key(&id) {
                problems.push(format!("{id}: the server's or the folder's version is used"));
                continue;
            }
            std::fs::copy(&p.path, dir.join(format!("m-{id}.bhmod")))?;
            config.insert(id.clone(), c);
            placed.push(Placed { id, origin: "mine" });
        }
    }
    let body = toml::to_string(&config).expect("mods.toml always serialises");
    std::fs::write(dir.join(mods::CONFIG_FILE), body)?;
    Ok((placed, problems))
}

/// A native mod of a session and the libraries to load into the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeMod {
    pub id: String,
    /// `client/native/armeabi-v7a/*.so`, extracted for packages (below the folder given to [`native_libs`]).
    pub libs: Vec<PathBuf>,
}

/// The id a session entry (`m-<id>.bhmod`, `s00-<id>.bhmod`, `d00-<id>/`) stands for.
fn entry_id(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".bhmod").unwrap_or(name);
    if let Some(id) = stem.strip_prefix("m-") {
        return Some(id);
    }
    let b = stem.as_bytes();
    (b.len() > 4 && matches!(b[0], b's' | b'd') && b[1].is_ascii_digit() && b[2].is_ascii_digit() && b[3] == b'-')
        .then(|| &stem[4..])
}

/// The native libraries of the mods placed in `dir` by [`build`] that were granted `native` (the player's
/// confirmation, the index listing and "never allow native" were settled when the grant was made). Packages are
/// unpacked below `extract_root`. Returns the mods and problems (a granted mod without a library for this ABI).
pub fn native_libs(dir: &Path, extract_root: &Path) -> (Vec<NativeMod>, Vec<String>) {
    let config = mods::read_config(dir).unwrap_or_default();
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = entry_id(&name) else { continue };
        if !config.get(id).is_some_and(|c| c.enabled && c.grant.iter().any(|g| g == "native")) {
            continue;
        }
        let path = e.path();
        let libs = if path.is_dir() {
            let lib_dir = path.join("client/native").join(bhmod_client::native::ABI);
            let mut v: Vec<PathBuf> = std::fs::read_dir(lib_dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|f| f.path())
                .filter(|p| p.extension().is_some_and(|x| x == "so"))
                .collect();
            v.sort();
            Ok(v)
        } else {
            let sha = std::fs::read(&path).map(|b| bhmod_proto::hex(&bhmod_client::cache::sha256(&b)));
            sha.and_then(|sha| bhmod_client::native::extract(&path, &sha, bhmod_client::native::ABI, extract_root))
        };
        match libs {
            Ok(l) if !l.is_empty() => found.push(NativeMod { id: id.to_string(), libs: l }),
            Ok(_) => problems.push(format!("{id}: no {} library for native code", bhmod_client::native::ABI)),
            Err(e) => problems.push(format!("{id}: cannot unpack the native library: {e}")),
        }
    }
    (found, problems)
}
