//! What the running game's mod loader reports: `bhpc-run` prints `@mods [...]` (a JSON list, see
//! `bhpc_mods::view::status_line`) on stdout whenever a mod's state or time changes; the session keeps the last one
//! for the Mods screens (running, turned off for its budget, failed, time used). The loader's log lines are the
//! game log's lines that start with [`LOG_PREFIX`].

use serde::{Deserialize, Serialize};

use super::Overview;

/// The status line's prefix.
pub const PREFIX: &str = "@mods ";
/// Log lines of the mod loader in the game's log.
pub const LOG_PREFIX: &str = "[mods] ";

/// One mod of the running session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveMod {
    pub id: String,
    pub version: String,
    /// `running`, `loaded`, `budget` or `failed`.
    pub state: String,
    /// `mine`, `dev` or `server`.
    pub origin: String,
    /// Milliseconds of mod code in the last whole second.
    pub ms: f64,
    /// Longest single call.
    pub peak_ms: f64,
    pub calls: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The mods of a status line, `None` for other lines.
pub fn parse(line: &str) -> Option<Vec<LiveMod>> {
    serde_json::from_str(line.trim_end().strip_prefix(PREFIX)?).ok()
}

/// Puts the live state of the running game onto the installed mods: `running`, `budget` or `failed` replace the
/// file-based status of mods the game runs now (a server's version of an installed id does not count).
pub fn apply(ov: &mut Overview, live: &[LiveMod]) {
    for m in &mut ov.mods {
        let Some(l) = live.iter().find(|l| l.id == m.id && l.origin == "mine") else { continue };
        if matches!(l.state.as_str(), "running" | "budget" | "failed") {
            m.status = l.state.clone();
            m.reason = l.reason.clone();
        }
        m.live = Some(l.clone());
    }
    ov.attention = ov.mods.iter().filter(|m| super::ATTENTION.contains(&m.status.as_str())).count();
}

/// The runner's mod flags for these settings: developer folders (developer mode only) and the native block.
pub fn args(prefs: &crate::prefs::Prefs) -> Vec<String> {
    let mut a = Vec::new();
    if prefs.mods_developer {
        for f in &prefs.mods_dev_folders {
            a.extend(["--mods-dev".to_string(), f.clone()]);
        }
    }
    if prefs.mods_block_native {
        a.push("--mods-block-native".to_string());
    }
    a
}
