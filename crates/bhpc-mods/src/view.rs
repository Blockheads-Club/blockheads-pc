//! What the in-game mod UI shows, as plain data the glue publishes from the game's main thread and the window
//! thread draws ([`crate::ui`]); and what the player answers ([`UiCmd`]).

use std::time::Instant;

use crate::host::{LogLine, ModStatus};
use crate::join::{JoinFlow, Refusal, Stage};

/// One mod in the join prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinMod {
    pub name: String,
    pub version: String,
    pub size: u64,
    pub have: u64,
    pub cached: bool,
    pub done: bool,
    pub native: bool,
    pub refused: bool,
    /// Short permission names (`channels`, `world:read`, `players:chat`, `net-http`).
    pub permissions: Vec<String>,
    pub listed: bool,
}

/// The join prompt's stage.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinStage {
    Prompt,
    Downloading,
    /// The text of the failure.
    Failed(String),
    /// The server refused the join (`REJECT_TEXT`): text and optional link.
    Rejected(String, Option<String>),
}

/// The join prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinView {
    pub server: String,
    pub stage: JoinStage,
    pub mods: Vec<JoinMod>,
    pub required: bool,
    /// `(bytes held, bytes to download)`.
    pub progress: (u64, u64),
}

impl JoinView {
    /// The prompt for a flow, `None` when nothing is to be shown (no prompt yet, joined, vanilla).
    pub fn of(f: &JoinFlow) -> Option<JoinView> {
        let stage = match f.stage() {
            Stage::Prompt => JoinStage::Prompt,
            Stage::Downloading => JoinStage::Downloading,
            Stage::Failed(e) => JoinStage::Failed(e.to_string()),
            _ => return None,
        };
        Some(JoinView {
            server: f.server_name().to_string(),
            stage,
            mods: f
                .entries()
                .iter()
                .map(|e| {
                    let p = &e.info.permissions;
                    let mut perms = Vec::new();
                    if p.world != "none" && !p.world.is_empty() {
                        perms.push(format!("world:{}", p.world));
                    }
                    perms.extend(p.players.iter().map(|x| format!("players:{x}")));
                    if !p.net_http.is_empty() {
                        perms.push("net-http".into());
                    }
                    if p.channels {
                        perms.push("channels".into());
                    }
                    JoinMod {
                        name: e.name(),
                        version: e.info.version.clone(),
                        size: e.info.size,
                        have: e.have,
                        cached: e.cached,
                        done: e.done,
                        native: p.native,
                        refused: e.refusal.is_some(),
                        permissions: perms,
                        listed: e.listed,
                    }
                })
                .collect(),
            required: f.server().is_some_and(|s| s.client_required),
            progress: f.progress(),
        })
    }

    /// A rejection to show.
    pub fn rejected(server: &str, text: &str, url: Option<&str>) -> JoinView {
        JoinView {
            server: server.to_string(),
            stage: JoinStage::Rejected(text.to_string(), url.map(str::to_string)),
            mods: Vec::new(),
            required: false,
            progress: (0, 0),
        }
    }

    /// True when "Download and join" may be pressed (`native_ok`: the red box is ticked).
    pub fn can_accept(&self, native_ok: bool) -> bool {
        self.stage == JoinStage::Prompt
            && self.mods.iter().all(|m| !m.refused)
            && (native_ok || self.mods.iter().all(|m| !m.native))
    }
}

/// Why a [`Refusal`] happened, in words.
pub fn refusal_text(r: &Refusal) -> &'static str {
    match r {
        Refusal::NativeNotListed => "Native code that is not on the public mod list",
        Refusal::NativeBlocked => "Native code is turned off in Settings",
    }
}

/// Everything the mod UI shows.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub join: Option<JoinView>,
    /// Mods of the running session with where they came from (`mine`, `dev`, `server`).
    pub mods: Vec<(ModStatus, &'static str)>,
    pub log: Vec<LogLine>,
    /// Short notices with the time they appeared.
    pub toasts: Vec<(Instant, String)>,
    /// The server the session belongs to (`None`: single-player or no session).
    pub server: Option<String>,
    /// Developer folder mods are loaded (F5 reloads them).
    pub dev: bool,
}

/// What the player did in the mod UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCmd {
    /// Join prompt answer.
    Decide { accept: bool, native_ok: bool },
    /// Stop a download and leave the server.
    Cancel,
    /// Close a failure or rejection notice.
    Dismiss,
    /// Reload the session's mods (developer mode).
    Reload,
}

#[derive(serde::Serialize)]
struct StatusItem<'a> {
    id: &'a str,
    version: &'a str,
    state: &'a str,
    origin: &'a str,
    ms: f64,
    peak_ms: f64,
    calls: u64,
    reason: Option<&'a str>,
}

/// The `@mods [...]` line `bhpc-run` prints for the launcher (JSON list of the session's mods).
pub fn status_line(mods: &[(ModStatus, &'static str)]) -> String {
    let items: Vec<StatusItem<'_>> = mods
        .iter()
        .map(|(s, o)| StatusItem {
            id: &s.id,
            version: &s.version,
            state: s.state,
            origin: o,
            ms: s.usage.ms_last_second,
            peak_ms: s.usage.peak_ms,
            calls: s.usage.calls,
            reason: s.reason.as_deref(),
        })
        .collect();
    format!("@mods {}", serde_json::to_string(&items).expect("status serialises"))
}
