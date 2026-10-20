//! The "Your pic" picture picker: the game's `startActivityForResult(ACTION_PICK images)` answered with a file dialog
//! of the desktop.
//!
//! The dialog is a helper program: `zenity` or `kdialog` on Linux (KDE's first under KDE), PowerShell's
//! `OpenFileDialog` on Windows, `osascript` on macOS. It runs on a thread of its own so the game keeps drawing; the UI
//! loop takes the answers with [`Pickers::poll`] and passes them to the game as `RESULT_OK` with a `file://` URI, or
//! `RESULT_CANCELED`. `BHPC_PICK_IMAGE` replaces the dialog (tests, headless runs): its value is the chosen file, an
//! empty value cancels.

use bhpc_jni::{ActivityRequest, RESULT_CANCELED, RESULT_OK};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};

/// What the user did with the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    File(PathBuf),
    Cancelled,
    /// No dialog program could be started.
    Unavailable,
}

/// A dialog program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub program: String,
    pub args: Vec<String>,
}

fn cand(program: &str, args: &[&str]) -> Candidate {
    Candidate { program: program.into(), args: args.iter().map(|a| a.to_string()).collect() }
}

const TITLE: &str = "Choose a picture";

/// The dialog programs to try on this system, best first; `desktop` is `XDG_CURRENT_DESKTOP`.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn candidates(desktop: &str) -> Vec<Candidate> {
    let zenity = cand(
        "zenity",
        &[
            "--file-selection",
            &format!("--title={TITLE}"),
            "--file-filter=Pictures | *.png *.jpg *.jpeg *.PNG *.JPG *.JPEG",
        ],
    );
    let kdialog = cand("kdialog", &["--title", TITLE, "--getopenfilename", ".", "image/png image/jpeg"]);
    if desktop.to_ascii_lowercase().contains("kde") {
        vec![kdialog, zenity]
    } else {
        vec![zenity, kdialog]
    }
}

/// The dialog programs to try on this system.
#[cfg(target_os = "macos")]
pub fn candidates(_desktop: &str) -> Vec<Candidate> {
    let script =
        format!("POSIX path of (choose file with prompt \"{TITLE}\" of type {{\"public.png\", \"public.jpeg\"}})");
    vec![cand("osascript", &["-e", &script])]
}

/// The dialog programs to try on this system.
#[cfg(windows)]
pub fn candidates(_desktop: &str) -> Vec<Candidate> {
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
         $d.Title = '{TITLE}'; $d.Filter = 'Pictures|*.png;*.jpg;*.jpeg'; \
         if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }} else {{ exit 1 }}"
    );
    vec![cand("powershell", &["-NoProfile", "-STA", "-Command", &script])]
}

/// Run the first candidate that starts: its first output line is the chosen file, a failure exit or no output is a
/// cancel.
pub fn run(cands: &[Candidate]) -> Pick {
    for c in cands {
        let Ok(out) = Command::new(&c.program).args(&c.args).output() else { continue };
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next().unwrap_or("").trim();
        return if out.status.success() && !line.is_empty() {
            Pick::File(PathBuf::from(line))
        } else {
            Pick::Cancelled
        };
    }
    Pick::Unavailable
}

/// Ask the user for a picture: `fixed` (the value of `BHPC_PICK_IMAGE`) if set, else the first of `dialogs` that
/// starts (normally [`candidates`]).
pub fn pick_image(fixed: Option<std::ffi::OsString>, dialogs: &[Candidate]) -> Pick {
    match fixed {
        Some(v) if v.is_empty() => Pick::Cancelled,
        Some(v) => Pick::File(PathBuf::from(v)),
        None => run(dialogs),
    }
}

/// The game's answer for `pick`: result code and data URI.
pub fn result_of(pick: &Pick) -> (i32, Option<String>) {
    match pick {
        Pick::File(p) => (RESULT_OK, Some(bhpc_jni::file_uri(&p.to_string_lossy()))),
        Pick::Cancelled | Pick::Unavailable => (RESULT_CANCELED, None),
    }
}

/// The message for the user, if the pick needs one.
pub fn message(pick: &Pick) -> Option<&'static str> {
    (*pick == Pick::Unavailable).then_some("NO FILE DIALOG: INSTALL ZENITY OR KDIALOG")
}

/// Dialogs in progress.
pub struct Pickers {
    tx: Sender<(i32, Pick)>,
    rx: Receiver<(i32, Pick)>,
}

impl Default for Pickers {
    fn default() -> Self {
        let (tx, rx) = channel();
        Pickers { tx, rx }
    }
}

impl Pickers {
    /// Start answering `req` with `pick` on a thread of its own: a picture request gets the dialog, any other
    /// activity (that the PC does not have) is cancelled at once.
    pub fn start(&self, req: &ActivityRequest, pick: Box<dyn FnOnce() -> Pick + Send>) {
        let (tx, code) = (self.tx.clone(), req.code);
        if !req.wants_image() {
            let _ = tx.send((code, Pick::Cancelled));
            return;
        }
        std::thread::spawn(move || {
            let _ = tx.send((code, pick()));
        });
    }

    /// Answers that are ready: request code and pick.
    pub fn poll(&self) -> Vec<(i32, Pick)> {
        self.rx.try_iter().collect()
    }
}
