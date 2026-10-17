//! The first-run flow as a plain state machine; the GUI renders it and calls these methods.

use crate::apk::{self, ApkInfo};
use crate::config::{LaunchConfig, LauncherError};
use crate::import::{self, Existing, Report};
use crate::keys::{Action, Binding};
use std::path::{Path, PathBuf};

/// Wizard pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Apk,
    DataDir,
    Keys,
    Window,
    Import,
    Done,
}

impl Step {
    /// Following page (`Done` stays).
    pub fn next(self) -> Step {
        match self {
            Step::Apk => Step::DataDir,
            Step::DataDir => Step::Keys,
            Step::Keys => Step::Window,
            Step::Window => Step::Import,
            Step::Import | Step::Done => Step::Done,
        }
    }

    /// Previous page (`Apk` stays).
    pub fn back(self) -> Step {
        match self {
            Step::Apk | Step::DataDir => Step::Apk,
            Step::Keys => Step::DataDir,
            Step::Window => Step::Keys,
            Step::Import => Step::Window,
            Step::Done => Step::Import,
        }
    }
}

/// Wizard state.
#[derive(Debug)]
pub struct Wizard {
    pub step: Step,
    pub config: LaunchConfig,
    pub apk_info: Option<ApkInfo>,
    /// Last user-visible error.
    pub error: Option<String>,
    pub import_report: Option<Report>,
    config_path: PathBuf,
}

impl Wizard {
    /// Starts from `existing` (re-configuration) or from defaults (first run).
    pub fn new(existing: Option<LaunchConfig>, config_path: PathBuf, default_data_dir: PathBuf) -> Wizard {
        let first = existing.is_none();
        Wizard {
            step: if first { Step::Apk } else { Step::Done },
            config: existing.unwrap_or_else(|| LaunchConfig::new("", default_data_dir)),
            apk_info: None,
            error: None,
            import_report: None,
            config_path,
        }
    }

    fn record<T>(&mut self, r: Result<T, LauncherError>) -> Option<T> {
        match r {
            Ok(v) => {
                self.error = None;
                Some(v)
            }
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    /// The user picked an APK. Stays on the page when it is not a supported build.
    pub fn pick_apk(&mut self, path: &Path) -> bool {
        match apk::validate(path, self.config.force) {
            Ok(info) => {
                self.config.apk = path.to_path_buf();
                self.apk_info = Some(info);
                self.error = None;
                true
            }
            Err(e) => {
                self.apk_info = None;
                self.error = Some(LauncherError::Apk(e).to_string());
                false
            }
        }
    }

    /// Sets the data directory.
    pub fn set_data_dir(&mut self, dir: PathBuf) {
        self.config.data_dir = dir;
    }

    /// Rebinds an action; refuses a binding another action already uses.
    pub fn rebind(&mut self, action: Action, text: &str) -> bool {
        let b = match Binding::parse(text) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(e.to_string());
                return false;
            }
        };
        if let Some(other) = Action::ALL.iter().find(|a| **a != action && self.config.keys.get(**a) == b) {
            self.error = Some(format!("{b} is already used by {}", other.label()));
            return false;
        }
        self.config.keys.set(action, b);
        self.error = None;
        true
    }

    /// Imports Android saves into the chosen data directory.
    pub fn import(&mut self, src: &Path, policy: Existing) -> bool {
        let r =
            import::import_saves(src, &self.config.data_dirs(), policy).map_err(|e| LauncherError::Io(e.to_string()));
        self.import_report = self.record(r);
        self.import_report.is_some()
    }

    /// Whether the current page may be left forward.
    pub fn can_advance(&self) -> bool {
        match self.step {
            Step::Apk => self.apk_info.is_some(),
            Step::Window => self.config.window.validate().is_ok(),
            _ => true,
        }
    }

    /// Goes to the next page if allowed.
    pub fn advance(&mut self) {
        if self.can_advance() {
            self.step = self.step.next();
        }
    }

    /// Goes to the previous page.
    pub fn back(&mut self) {
        self.step = self.step.back();
    }

    /// Validates and saves the config; returns it for launching.
    pub fn finish(&mut self) -> Option<LaunchConfig> {
        let r = self.config.validate().and_then(|_| self.config.save(&self.config_path));
        self.record(r).map(|_| self.config.clone())
    }
}
