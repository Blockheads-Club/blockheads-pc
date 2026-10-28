//! The mods' HUD on PC (API `ui`): widgets in the top corners, toasts, notices and dialogs, kept on the main thread
//! by the bridge and drawn by [`crate::ui::ModUi`] in the in-game kit. On-screen buttons (`ui.set-button`) are a
//! touch control and stay `unsupported` on desktop (`ui.has-touch` is false); desktop mods bind a key instead.
//!
//! Dialogs never block the game thread: the mod gets its answer later as `<mod-id>:ui-action`
//! ([`bhmod_core::ModHost::ui_result`]). One dialog shows at a time; the others wait in order.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bhmod_core::ui::{Dialog, NoticeLevel, Widget};

/// How long a toast stays.
pub const TOAST_TIME: Duration = Duration::from_secs(4);
/// How long a notice stays.
pub const NOTICE_TIME: Duration = Duration::from_secs(10);
/// Notices shown at once (older ones go).
pub const MAX_NOTICES: usize = 3;

/// A widget of a mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModWidget {
    pub mod_id: String,
    pub id: String,
    pub widget: Widget,
}

/// A notice (`ui.notice`, also the loader's own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub at: Instant,
    pub level: NoticeLevel,
    pub text: String,
}

/// A dialog waiting for the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDialog {
    pub mod_id: String,
    pub id: u32,
    pub dialog: Dialog,
}

/// What the mods show.
#[derive(Debug, Default)]
pub struct Hud {
    widgets: Vec<ModWidget>,
    toasts: Vec<(Instant, String)>,
    notices: Vec<Notice>,
    dialogs: Vec<ModDialog>,
    /// Mods whose widgets the player hid, kept in `hidden_file`.
    hidden: BTreeSet<String>,
    hidden_file: Option<PathBuf>,
    /// Bumped on every change (the picture is redrawn).
    pub serial: u64,
}

impl Hud {
    /// The file of the mods whose widgets the player hid (one id per line).
    pub fn hidden_path(data_dir: &Path) -> PathBuf {
        data_dir.join("mod-hidden-widgets.txt")
    }

    /// A HUD that keeps the hidden mods in `file`.
    pub fn new(file: Option<PathBuf>) -> Hud {
        let hidden = file
            .as_deref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        Hud { hidden, hidden_file: file, ..Hud::default() }
    }

    fn bump(&mut self) {
        self.serial += 1;
    }

    pub fn set_widget(&mut self, mod_id: &str, id: &str, widget: Widget) {
        let w = ModWidget { mod_id: mod_id.into(), id: id.into(), widget };
        match self.widgets.iter_mut().find(|x| x.mod_id == mod_id && x.id == id) {
            Some(x) => *x = w,
            None => self.widgets.push(w),
        }
        self.bump();
    }

    pub fn remove_widget(&mut self, mod_id: &str, id: &str) {
        self.widgets.retain(|x| !(x.mod_id == mod_id && x.id == id));
        self.bump();
    }

    pub fn toast(&mut self, text: &str, now: Instant) {
        self.toasts.push((now, text.to_string()));
        self.bump();
    }

    pub fn notice(&mut self, level: NoticeLevel, text: &str, now: Instant) {
        self.notices.push(Notice { at: now, level, text: text.to_string() });
        if self.notices.len() > MAX_NOTICES {
            self.notices.remove(0);
        }
        self.bump();
    }

    pub fn show_dialog(&mut self, mod_id: &str, id: u32, dialog: Dialog) {
        self.dialogs.push(ModDialog { mod_id: mod_id.into(), id, dialog });
        self.bump();
    }

    /// The mod closed a dialog; true when it was still open.
    pub fn close_dialog(&mut self, id: u32) -> bool {
        let before = self.dialogs.len();
        self.dialogs.retain(|d| d.id != id);
        self.bump();
        before != self.dialogs.len()
    }

    /// The player answered the dialog `id` (the caller tells the mod host); false when it is not open.
    pub fn answered(&mut self, id: u32) -> bool {
        self.close_dialog(id)
    }

    /// The mod is gone: its widgets and dialogs go (its toasts and notices run out on their own).
    pub fn clear_mod(&mut self, mod_id: &str) {
        self.widgets.retain(|x| x.mod_id != mod_id);
        self.dialogs.retain(|d| d.mod_id != mod_id);
        self.bump();
    }

    /// Everything goes (the session ended). The dialogs that were open, for `CLOSED` answers.
    pub fn clear(&mut self) -> Vec<u32> {
        self.widgets.clear();
        self.toasts.clear();
        self.notices.clear();
        self.bump();
        self.dialogs.drain(..).map(|d| d.id).collect()
    }

    /// Drops toasts and notices that ran out; true when something went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let n = self.toasts.len() + self.notices.len();
        self.toasts.retain(|(at, _)| now.saturating_duration_since(*at) < TOAST_TIME);
        self.notices.retain(|x| now.saturating_duration_since(x.at) < NOTICE_TIME);
        let gone = n != self.toasts.len() + self.notices.len();
        if gone {
            self.bump();
        }
        gone
    }

    /// The player shows or hides a mod's widgets (kept on the device).
    pub fn set_hidden(&mut self, mod_id: &str, hidden: bool) -> std::io::Result<()> {
        if hidden {
            self.hidden.insert(mod_id.to_string());
        } else {
            self.hidden.remove(mod_id);
        }
        self.bump();
        match &self.hidden_file {
            Some(f) => std::fs::write(f, self.hidden.iter().map(|m| format!("{m}\n")).collect::<String>()),
            None => Ok(()),
        }
    }

    pub fn is_hidden(&self, mod_id: &str) -> bool {
        self.hidden.contains(mod_id)
    }

    /// True when the mod has widgets (the settings screen offers to hide them).
    pub fn has_widgets(&self, mod_id: &str) -> bool {
        self.widgets.iter().any(|w| w.mod_id == mod_id)
    }

    /// The widgets to draw (hidden mods left out), in the order they were first set.
    pub fn widgets(&self) -> Vec<ModWidget> {
        self.widgets.iter().filter(|w| !self.hidden.contains(&w.mod_id)).cloned().collect()
    }

    pub fn toasts(&self) -> Vec<(Instant, String)> {
        self.toasts.clone()
    }

    pub fn notices(&self) -> Vec<Notice> {
        self.notices.clone()
    }

    /// The dialog that shows (the oldest open one).
    pub fn dialog(&self) -> Option<ModDialog> {
        self.dialogs.first().cloned()
    }
}
