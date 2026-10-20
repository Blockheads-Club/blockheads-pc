//! The desktop side of what the game asks of the phone around it ([`bhpc_jni::SystemHooks`]): links and mail open
//! in the user's default programs, copied text goes to the clipboard, notices show over the game, the help page
//! opens in the browser, and camera photos are reported where they landed (the user's Pictures folder, see
//! [`pictures_dir`] and `bhpc_vfs::VfsConfig::pictures`).
//!
//! The opener is `BHPC_OPENER` (a command, for tests and special setups), else `BROWSER` (headless runs set it to
//! `true`, so nothing opens), else the platform's (`xdg-open`, `open`, the Windows URL handler).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(windows)]
const PLATFORM_OPENER: &[&str] = &["rundll32", "url.dll,FileProtocolHandler"];
#[cfg(target_os = "macos")]
const PLATFORM_OPENER: &[&str] = &["open"];
#[cfg(not(any(windows, target_os = "macos")))]
const PLATFORM_OPENER: &[&str] = &["xdg-open"];

/// How links and files are handed to the desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opener {
    /// Program and leading arguments; `%s` in an argument is replaced by the target, else it is appended.
    pub argv: Vec<String>,
}

impl Opener {
    /// The opener the environment asks for (`var` reads one variable).
    pub fn from_env(var: &dyn Fn(&str) -> Option<String>) -> Opener {
        let pick = |name: &str| var(name).filter(|v| !v.trim().is_empty());
        // `BROWSER` may list several programs separated by `:`; the first one is used.
        let cmd = pick("BHPC_OPENER")
            .or_else(|| pick("BROWSER").map(|b| b.split(':').next().unwrap_or_default().to_string()));
        match cmd {
            Some(c) => Opener { argv: c.split_whitespace().map(str::to_string).collect() },
            None => Opener::platform(),
        }
    }

    /// The platform's own opener.
    pub fn platform() -> Opener {
        Opener { argv: PLATFORM_OPENER.iter().map(|s| s.to_string()).collect() }
    }

    /// The command line that opens `target`.
    pub fn command_line(&self, target: &str) -> Vec<String> {
        let mut out: Vec<String> = self.argv.iter().map(|a| a.replace("%s", target)).collect();
        if !self.argv.iter().any(|a| a.contains("%s")) {
            out.push(target.to_string());
        }
        out
    }

    /// Start the opener for `target` without waiting for it (a thread reaps it). False if it could not start.
    pub fn open(&self, target: &str) -> bool {
        let line = self.command_line(target);
        let Some((prog, args)) = line.split_first() else { return false };
        let child = Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::null()).spawn();
        match child {
            Ok(mut c) => {
                std::thread::spawn(move || c.wait());
                true
            }
            Err(e) => {
                eprintln!("[desktop] cannot start {prog}: {e}");
                false
            }
        }
    }
}

/// The host folder for the game's camera photos: `XDG_PICTURES_DIR`, else the platform's Pictures folder (`dirs`),
/// else `<home>/Pictures`.
pub fn pictures_dir(
    var: &dyn Fn(&str) -> Option<String>,
    platform: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    var("XDG_PICTURES_DIR")
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .or(platform)
        .or_else(|| home.map(|h| h.join("Pictures")))
}

/// [`pictures_dir`] of this process.
pub fn user_pictures_dir() -> Option<PathBuf> {
    pictures_dir(&|k| std::env::var(k).ok(), dirs::picture_dir(), dirs::home_dir())
}

/// Guest folders that are the host Pictures folder (as mounted by `bhpc_vfs`).
const PICTURE_ROOTS: [&str; 2] = ["Pictures/", "DCIM/"];
/// Guest aliases of the external storage.
const EXTERNAL: [&str; 4] = ["/sdcard/", "/mnt/sdcard/", "/storage/emulated/0/", "/storage/emulated/legacy/"];

/// The host path of a guest picture path below one of the shared picture folders.
pub fn host_picture(pictures: &Path, guest: &str) -> Option<PathBuf> {
    let rest = EXTERNAL.iter().find_map(|e| guest.strip_prefix(e))?;
    let rel = PICTURE_ROOTS.iter().find_map(|r| rest.strip_prefix(r))?;
    Some(pictures.join(rel))
}

/// Where a notice or copied text goes in this run.
pub type Sink = Box<dyn Fn(&str) + Send + Sync>;

/// [`bhpc_jni::SystemHooks`] for the desktop.
pub struct Desktop {
    pub opener: Opener,
    /// Folder for the HTML pages the game shows (help, credits); one sub-folder per page.
    pub pages: PathBuf,
    /// The host Pictures folder behind the guest's (`None`: photos stay in the emulated storage).
    pub pictures: Option<PathBuf>,
    /// Puts text on the clipboard.
    pub copy: Sink,
    /// Shows a short notice over the game.
    pub notice: Sink,
    next_page: AtomicU32,
}

impl Desktop {
    pub fn new(opener: Opener, pages: PathBuf, pictures: Option<PathBuf>, copy: Sink, notice: Sink) -> Desktop {
        Desktop { opener, pages, pictures, copy, notice, next_page: AtomicU32::new(0) }
    }

    /// Write a page and its resources into a fresh folder below [`Desktop::pages`]; the page's path.
    pub fn write_page(&self, html: &str, files: &[(String, Vec<u8>)]) -> std::io::Result<PathBuf> {
        let n = self.next_page.fetch_add(1, Ordering::SeqCst);
        let dir = self.pages.join(format!("{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        for (name, data) in files {
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap_or(&dir))?;
            std::fs::write(p, data)?;
        }
        let page = dir.join("index.html");
        std::fs::write(&page, html)?;
        Ok(page)
    }
}

/// `file://` URL of a local path.
pub fn file_url(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    let s = if s.starts_with('/') { s } else { format!("/{s}") };
    format!("file://{}", s.replace(' ', "%20"))
}

impl bhpc_jni::SystemHooks for Desktop {
    fn open_url(&self, url: &str) -> bool {
        eprintln!("[desktop] open {url}");
        self.opener.open(url)
    }

    fn show_html(&self, html: &str, files: &[(String, Vec<u8>)]) -> bool {
        match self.write_page(html, files) {
            Ok(page) => {
                eprintln!("[desktop] page {}", page.display());
                self.opener.open(&file_url(&page))
            }
            Err(e) => {
                eprintln!("[desktop] cannot write the page: {e}");
                false
            }
        }
    }

    fn set_clipboard(&self, text: &str) {
        eprintln!("[desktop] copied {} characters", text.chars().count());
        (self.copy)(text);
    }

    fn notify(&self, text: &str) {
        (self.notice)(text);
    }

    fn picture_saved(&self, path: &str) {
        let host = self.pictures.as_deref().and_then(|p| host_picture(p, path));
        let shown = host.as_deref().map_or(path.to_string(), |h| h.display().to_string());
        eprintln!("[desktop] photo saved: {shown}");
        (self.notice)(&format!("PHOTO SAVED: {shown}"));
    }
}
