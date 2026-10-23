//! What the in-game mod UI shows, as plain data the glue publishes from the game's main thread and the window
//! thread draws ([`crate::ui`]); and what the player answers ([`UiCmd`]).

use std::time::Instant;

use crate::host::{LogLine, ModStatus};
use bhmod_client::session::Failure;
use bhmod_client::verify::Refusal as PackageRefusal;
use bhmod_proto::hex;

use crate::join::{Entry, JoinFlow, Refusal, Stage};

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
    /// The server needs this mod (it cannot be switched off).
    pub required: bool,
    /// The player takes this mod (a switched-off optional mod is not downloaded or run).
    pub enabled: bool,
    /// Short permission names (`channels`, `world:read`, `players:chat`, `net-http`).
    pub permissions: Vec<String>,
    pub listed: bool,
    /// The mod also has a server half ("Client + server"); else it only runs here.
    pub with_server: bool,
}

/// What went wrong, for the layout of the failure notice.
#[derive(Debug, Clone, PartialEq)]
pub enum FailKind {
    /// A download stopped or the server could not send a file (`percent` of that file was there).
    Download {
        name: String,
        percent: u8,
    },
    /// The file's SHA-256 is not the one listed (`expected`: 64 hex characters).
    Check {
        name: String,
        version: String,
        expected: String,
    },
    /// The server needs a newer mod loader (or API) than this one.
    Newer,
    Other,
}

/// A failed join.
#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub kind: FailKind,
    /// The explanation under the headline.
    pub text: String,
}

/// The join prompt's stage.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinStage {
    Prompt,
    Downloading,
    Failed(Fail),
    /// The server refused the join (`REJECT_TEXT`): text and optional link.
    Rejected(String, Option<String>),
}

/// The join prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinView {
    pub server: String,
    /// `host:port` as the player knows the server.
    pub address: String,
    pub stage: JoinStage,
    pub mods: Vec<JoinMod>,
    pub required: bool,
    /// `(bytes held, bytes to download)`.
    pub progress: (u64, u64),
    /// Seconds left of the download at the pace so far.
    pub eta: Option<u64>,
}

impl JoinView {
    /// The prompt for a flow, `None` when nothing is to be shown (no prompt yet, joined, vanilla).
    pub fn of(f: &JoinFlow, now: Instant) -> Option<JoinView> {
        let stage = match f.stage() {
            Stage::Prompt => JoinStage::Prompt,
            Stage::Downloading => JoinStage::Downloading,
            Stage::Failed(e) => JoinStage::Failed(fail_of(&e, f.server_name(), f.entries())),
            _ => return None,
        };
        Some(JoinView {
            server: f.server_name().to_string(),
            address: f.address().to_string(),
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
                        required: e.required(),
                        enabled: e.enabled,
                        permissions: perms,
                        listed: e.listed,
                        with_server: e.info.sides.iter().any(|s| s == "server"),
                    }
                })
                .collect(),
            required: f.required(),
            progress: f.progress(),
            eta: f.eta_secs(now),
        })
    }

    /// A rejection to show.
    pub fn rejected(server: &str, text: &str, url: Option<&str>) -> JoinView {
        JoinView {
            server: server.to_string(),
            address: String::new(),
            stage: JoinStage::Rejected(text.to_string(), url.map(str::to_string)),
            mods: Vec::new(),
            required: false,
            progress: (0, 0),
            eta: None,
        }
    }

    /// True when "Download and join" may be pressed (`native_ok`: the red box is ticked). Mods the player switched
    /// off do not count.
    pub fn can_accept(&self, native_ok: bool) -> bool {
        self.stage == JoinStage::Prompt
            && self.mods.iter().all(|m| !m.enabled || !m.refused)
            && (native_ok || self.mods.iter().all(|m| !m.enabled || !m.native))
    }
}

fn entry_named<'a>(entries: &'a [Entry], id: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| e.info.id == id)
}

/// The failure notice for a join failure.
fn fail_of(f: &Failure, server: &str, entries: &[Entry]) -> Fail {
    let named = |id: &str| entry_named(entries, id).map_or_else(|| id.to_string(), Entry::name);
    let percent = |id: &str| {
        entry_named(entries, id)
            .map_or(0, |e| (e.have.min(e.info.size) * 100).checked_div(e.info.size).unwrap_or(0) as u8)
    };
    match f {
        Failure::Download { id, code } => Fail {
            kind: FailKind::Download { name: named(id), percent: percent(id) },
            text: format!("The server could not send the file (error {code}). Nothing was saved and you didn't join."),
        },
        Failure::Stalled(id) => Fail {
            kind: FailKind::Download { name: named(id), percent: percent(id) },
            text: "The connection to the server was lost. Nothing was saved and you didn't join.".into(),
        },
        Failure::HashMismatch(id) => {
            let e = entry_named(entries, id);
            Fail {
                kind: FailKind::Check {
                    name: named(id),
                    version: e.map(|e| e.info.version.clone()).unwrap_or_default(),
                    expected: e.map(|e| hex(&e.info.sha256)).unwrap_or_default(),
                },
                text:
                    "It may be broken or changed. We deleted it and didn't run it. Ask the server owner to fix the mod."
                        .into(),
            }
        }
        Failure::Refused {
            refusal: PackageRefusal::NeedsNewerApi(_) | PackageRefusal::NeedsNewerLoader(_), ..
        } => Fail {
            kind: FailKind::Newer,
            text: format!("{server} needs a newer mod loader than this one. Update Blockheads PC, then join again."),
        },
        other => Fail { kind: FailKind::Other, text: other.to_string() },
    }
}

#[cfg(test)]
pub(crate) fn fail_for_tests(f: &Failure) -> Fail {
    fail_of(f, "S", &[])
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
    /// Switch the optional mod `0` (index in the prompt's list) on or off.
    Toggle(usize),
    /// Stop a download and leave the server.
    Cancel,
    /// Close a failure or rejection notice.
    Dismiss,
    /// Reload the session's mods (developer mode).
    Reload,
    /// Put this text on the clipboard.
    Copy(String),
    /// Say this line in the game's chat as if it had been typed (tests, developer tools).
    Say(String),
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
