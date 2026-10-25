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
    // Component by component: the guest's `/` must become the host's separator (Windows shows `\`).
    Some(rel.split('/').filter(|c| !c.is_empty()).fold(pictures.to_path_buf(), |p, c| p.join(c)))
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
    /// Shows a self-contained page in place of the game (the launcher's window); `None`: pages go to the browser.
    pub page: Option<Sink>,
    next_page: AtomicU32,
}

impl Desktop {
    pub fn new(opener: Opener, pages: PathBuf, pictures: Option<PathBuf>, copy: Sink, notice: Sink) -> Desktop {
        Desktop { opener, pages, pictures, copy, notice, page: None, next_page: AtomicU32::new(0) }
    }

    /// Pages go to `page` (self-contained, see [`inline_page`]) instead of the browser.
    pub fn with_page_sink(mut self, page: Sink) -> Desktop {
        self.page = Some(page);
        self
    }

    /// Write a page and its resources into a fresh folder below [`Desktop::pages`]; the page's path.
    pub fn write_page(&self, html: &str, files: &[(String, Vec<u8>)]) -> std::io::Result<PathBuf> {
        let n = self.next_page.fetch_add(1, Ordering::SeqCst);
        let dir = self.pages.join(format!("{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        for (name, data) in files {
            // Names come from the game: only plain relative paths inside the page's folder.
            if name.is_empty() || name.contains(['\\', ':']) || name.split('/').any(|c| c.is_empty() || c == "..") {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("bad page file {name:?}")));
            }
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap_or(&dir))?;
            std::fs::write(p, data)?;
        }
        let page = dir.join("index.html");
        std::fs::write(&page, html)?;
        Ok(page)
    }
}

/// Standard base64 of `data` (padded).
fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `text` with every quoted or `url(...)` reference to a file replaced by its `data:` URL.
fn inline_refs(text: &str, urls: &[(&str, String)]) -> String {
    let mut out = text.to_string();
    for (name, url) in urls {
        for (open, close) in [("\"", "\""), ("'", "'"), ("(", ")")] {
            out = out.replace(&format!("{open}{name}{close}"), &format!("{open}{url}{close}"));
        }
    }
    out
}

/// A page and its files as one HTML document: pictures become `data:` URLs, style sheets are added inline (the
/// launcher shows it without scripts or file access).
pub fn inline_page(html: &str, files: &[(String, Vec<u8>)]) -> String {
    let mime = |name: &str| match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "css" => "text/css",
        _ => "application/octet-stream",
    };
    let urls: Vec<(&str, String)> = files
        .iter()
        .filter(|(n, _)| mime(n) != "text/css")
        .map(|(n, d)| (n.as_str(), format!("data:{};base64,{}", mime(n), base64(d))))
        .collect();
    let styles: String = files
        .iter()
        .filter(|(n, _)| mime(n) == "text/css")
        .map(|(_, d)| format!("<style>{}</style>", inline_refs(&String::from_utf8_lossy(d), &urls)))
        .collect();
    format!("{styles}{}", inline_refs(html, &urls))
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
        if let Some(page) = &self.page {
            eprintln!("[desktop] page of {} bytes shown in the window", html.len());
            page(&inline_page(html, files));
            return true;
        }
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

    fn shows_pages(&self) -> bool {
        self.page.is_some()
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
