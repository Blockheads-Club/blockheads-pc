//! Key bindings of client mods (API 0.3.4 `input`): what each running mod declared with `input.bind`, and the
//! player's own keys for them, kept on the device (`<data>/mod-keys.toml`, one table per mod id, valid on every
//! server). The player rebinds in the loader's settings screen; an empty key clears a binding.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One declared action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    pub mod_id: String,
    pub action: String,
    pub default_key: String,
}

/// An action as the settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindRow {
    pub action: String,
    pub default_key: String,
    /// The key now (`None`: cleared by the player).
    pub key: Option<String>,
}

type Overrides = BTreeMap<String, BTreeMap<String, String>>;

/// The declared actions and the player's keys.
#[derive(Debug, Default)]
pub struct Binds {
    path: Option<PathBuf>,
    declared: Vec<Bind>,
    overrides: Overrides,
}

impl Binds {
    /// The file in the launcher's data folder.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("mod-keys.toml")
    }

    /// Bindings stored in `path` (missing or broken: none); changes are written back there.
    pub fn load(path: &Path) -> Binds {
        let overrides = std::fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
        Binds { path: Some(path.to_path_buf()), declared: Vec::new(), overrides }
    }

    /// A mod declares (or re-declares) an action.
    pub fn bind(&mut self, mod_id: &str, action: &str, default_key: &str) {
        self.unbind(mod_id, action);
        self.declared.push(Bind { mod_id: mod_id.into(), action: action.into(), default_key: default_key.into() });
    }

    /// A mod removes an action; false when it had none by that id.
    pub fn unbind(&mut self, mod_id: &str, action: &str) -> bool {
        let before = self.declared.len();
        self.declared.retain(|b| !(b.mod_id == mod_id && b.action == action));
        before != self.declared.len()
    }

    /// The mod is gone.
    pub fn clear_mod(&mut self, mod_id: &str) {
        self.declared.retain(|b| b.mod_id != mod_id);
    }

    fn current(&self, b: &Bind) -> Option<String> {
        match self.overrides.get(&b.mod_id).and_then(|m| m.get(&b.action)) {
            Some(k) if k.is_empty() => None,
            Some(k) => Some(k.clone()),
            None => Some(b.default_key.clone()),
        }
    }

    /// The key of an action now (the player's, else the declared default; `None`: cleared or not declared).
    pub fn key_of(&self, mod_id: &str, action: &str) -> Option<String> {
        self.declared.iter().find(|b| b.mod_id == mod_id && b.action == action).and_then(|b| self.current(b))
    }

    /// The `(mod, action)`s a key fires.
    pub fn find(&self, key: &str) -> Vec<(String, String)> {
        self.declared
            .iter()
            .filter(|b| self.current(b).as_deref() == Some(key))
            .map(|b| (b.mod_id.clone(), b.action.clone()))
            .collect()
    }

    /// The actions of a mod, in declaration order.
    pub fn rows(&self, mod_id: &str) -> Vec<BindRow> {
        self.declared
            .iter()
            .filter(|b| b.mod_id == mod_id)
            .map(|b| BindRow { action: b.action.clone(), default_key: b.default_key.clone(), key: self.current(b) })
            .collect()
    }

    /// The player sets a key (`None` clears it; the default key puts the default back) and it is stored.
    pub fn set_key(&mut self, mod_id: &str, action: &str, key: Option<&str>) -> std::io::Result<()> {
        let default = self.declared.iter().find(|b| b.mod_id == mod_id && b.action == action).map(|b| &b.default_key);
        let m = self.overrides.entry(mod_id.to_string()).or_default();
        match key {
            Some(k) if Some(&k.to_string()) == default => {
                m.remove(action);
            }
            k => {
                m.insert(action.to_string(), k.unwrap_or("").to_string());
            }
        }
        if m.is_empty() {
            self.overrides.remove(mod_id);
        }
        match &self.path {
            Some(p) => std::fs::write(p, toml::to_string(&self.overrides).expect("key bindings serialise")),
            None => Ok(()),
        }
    }
}
