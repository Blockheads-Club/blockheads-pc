//! Playing inside the launcher window: how the game runner (`bhpc-run --embed FILE`) and the launcher talk.
//!
//! * Frames go through a **frame file** in shared memory (`/dev/shm` or the runtime dir; any file works). The
//!   launcher creates it ([`FrameReader::create`]), the runner fills it ([`FrameWriter`]), and the launcher paints the
//!   newest frame into a native widget. The file has a small header and three pixel slots; the writer never touches
//!   the slot the header currently names, so the reader gets whole frames without locks (a reader that is two
//!   frames late detects that and skips). Pixels are BGRA, top row first, premultiplied (opaque), which is what
//!   cairo and most compositors take, unless the launcher asked for another [`Layout`] ([`Cmd::Layout`]): a view
//!   that draws with OpenGL takes the runner's read-back as it is (RGBA, bottom row first), which saves the runner
//!   the conversion. Each frame says its layout, so a change of mind takes effect with the next frame.
//! * Input goes the other way as text lines on the runner's stdin ([`Cmd`]): one command per line, in device pixels
//!   of the frame, HID usage ids for keys. The runner exits when stdin closes, so it never outlives the launcher.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Exit code of `bhpc-run --embed` when it cannot get a GL context without a window of its own: the launcher then
/// starts the game in a window of its own instead.
pub const EMBED_UNAVAILABLE_EXIT: i32 = 3;

const MAGIC: u32 = 0x3146_4842; // "BHF1"
const HEADER: u64 = 64;
const SLOTS: u64 = 3;
/// Largest frame the file can hold (8K).
pub const MAX_FRAME_BYTES: u64 = 8192 * 4608 * 4;

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], at: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, at)
}
#[cfg(unix)]
pub(crate) fn write_at(f: &File, buf: &[u8], at: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, at)
}
#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut at: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_read(f, buf, at)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        at += n as u64;
    }
    Ok(())
}
#[cfg(windows)]
pub(crate) fn write_at(f: &File, mut buf: &[u8], mut at: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_write(f, buf, at)?;
        buf = &buf[n..];
        at += n as u64;
    }
    Ok(())
}

fn slot_offset(slot: u64, cap: u64) -> u64 {
    HEADER + slot * cap
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().expect("4 bytes"))
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"))
}

/// How the pixels of a frame are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// BGRA, top row first (cairo's `ARGB32` on little-endian machines).
    #[default]
    Bgra,
    /// RGBA, bottom row first: what `glReadPixels` returns and `glTexSubImage2D` takes.
    GlRgba,
}

impl Layout {
    fn code(self) -> u32 {
        match self {
            Layout::Bgra => 0,
            Layout::GlRgba => 1,
        }
    }

    fn from_code(code: u32) -> Option<Layout> {
        match code {
            0 => Some(Layout::Bgra),
            1 => Some(Layout::GlRgba),
            _ => None,
        }
    }
}

/// Header: magic, version, slot capacity, sequence number of the newest frame (0 = none), its width, height, slot
/// and pixel layout.
struct Header {
    cap: u64,
    seq: u64,
    width: u32,
    height: u32,
    slot: u32,
    layout: u32,
}

fn read_header(f: &File) -> io::Result<Header> {
    let mut b = [0u8; 40];
    read_at(f, &mut b, 0)?;
    if u32_at(&b, 0) != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a frame file"));
    }
    Ok(Header {
        cap: u64_at(&b, 8),
        seq: u64_at(&b, 16),
        width: u32_at(&b, 24),
        height: u32_at(&b, 28),
        slot: u32_at(&b, 32),
        layout: u32_at(&b, 36),
    })
}

/// True when the writer has come round to the slot of frame `read` by the time `now` is the newest sequence number.
pub(crate) fn overwritten(read: u64, now: u64) -> bool {
    now >= read + SLOTS - 1
}

/// The directory for frame files: the user's runtime dir (tmpfs) when there is one, else the temp dir.
pub fn frame_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(std::env::temp_dir)
}

/// A fresh frame file name for this process.
pub fn frame_path(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("bhpc-frame-{}-{n}", std::process::id()))
}

/// Deletes frame files in `dir` whose launcher process (the pid in the name) is gone, e.g. after a crash or kill.
/// `alive` says whether a pid still runs. Returns how many files were removed.
pub fn clean_stale(dir: &Path, alive: &dyn Fn(u32) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let pid = name.strip_prefix("bhpc-frame-").and_then(|r| r.split('-').next()).and_then(|p| p.parse().ok());
            pid.is_some_and(|p| !alive(p))
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Whether a process with this pid runs (Linux: `/proc/<pid>`; elsewhere every pid counts as alive, so nothing is
/// cleaned up).
pub fn pid_alive(pid: u32) -> bool {
    !cfg!(target_os = "linux") || Path::new(&format!("/proc/{pid}")).exists()
}

/// One frame as the reader sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    pub layout: Layout,
}

/// The launcher's end: creates the file and reads the newest frame.
pub struct FrameReader {
    file: File,
    path: PathBuf,
    cap: u64,
    seen: u64,
    /// The whole file mapped into memory ([`FrameReader::read_with`]), made on first use.
    #[cfg(any(unix, windows))]
    map: Option<Map>,
    /// Where [`FrameReader::read_with`] copies frames without a mapping.
    #[cfg(not(any(unix, windows)))]
    scratch: Vec<u8>,
}

/// A read-only shared mapping of a frame file.
#[cfg(any(unix, windows))]
pub(crate) struct Map {
    ptr: *mut std::ffi::c_void,
    len: usize,
}

#[cfg(unix)]
impl Map {
    pub(crate) fn new(file: &File, len: u64) -> io::Result<Map> {
        use std::os::unix::io::AsRawFd;
        let len = len as usize;
        // SAFETY: a fresh shared read-only mapping of an open file; nothing else aliases it in this process.
        let ptr =
            unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Map { ptr, len })
    }
}

#[cfg(windows)]
impl Map {
    pub(crate) fn new(file: &File, len: u64) -> io::Result<Map> {
        Map::open(file, len, false)
    }

    /// A shared mapping the writer copies frames into (`WriteFile` of a whole frame into the temp file took 5 to 11 ms
    /// per 1080p frame, a `memcpy` about one).
    pub(crate) fn new_rw(file: &File, len: u64) -> io::Result<Map> {
        Map::open(file, len, true)
    }

    fn open(file: &File, len: u64, write: bool) -> io::Result<Map> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE, PAGE_READONLY, PAGE_READWRITE,
        };
        // SAFETY: a read-only section over an open file and one view of it; the section handle can be closed at once
        // (the view keeps it alive).
        unsafe {
            let section = CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                if write { PAGE_READWRITE } else { PAGE_READONLY },
                (len >> 32) as u32,
                len as u32,
                std::ptr::null(),
            );
            if section.is_null() {
                return Err(io::Error::last_os_error());
            }
            let view = MapViewOfFile(section, if write { FILE_MAP_WRITE } else { FILE_MAP_READ }, 0, 0, len as usize);
            let err = io::Error::last_os_error();
            CloseHandle(section);
            if view.Value.is_null() {
                return Err(err);
            }
            Ok(Map { ptr: view.Value, len: len as usize })
        }
    }

    /// Copies `px` to `at` of a writable mapping; the caller checked the range against the file size.
    fn write(&mut self, at: u64, px: &[u8]) {
        assert!(at as usize + px.len() <= self.len, "range outside the mapping");
        // SAFETY: inside a writable mapping owned by `self` (checked above); the reader only reads it.
        unsafe { std::ptr::copy_nonoverlapping(px.as_ptr(), self.ptr.cast::<u8>().add(at as usize), px.len()) };
    }
}

#[cfg(any(unix, windows))]
impl Map {
    /// `len` bytes at `at`; the caller checked the range against the file size.
    fn bytes(&self, at: u64, len: u64) -> &[u8] {
        let (at, len) = (at as usize, len as usize);
        assert!(at + len <= self.len, "range outside the mapping");
        // SAFETY: inside the mapping (checked above), which lives as long as `self`. The writer may change the bytes
        // meanwhile (another process); the reader detects that with the sequence number and drops the frame.
        unsafe { std::slice::from_raw_parts(self.ptr.cast::<u8>().add(at), len) }
    }
}

// SAFETY: the mapping is read-only and owned by one `FrameReader`; any thread may read it.
#[cfg(any(unix, windows))]
unsafe impl Send for Map {}

#[cfg(unix)]
impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what `new` mapped.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

#[cfg(windows)]
impl Drop for Map {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS};
        // SAFETY: unmaps exactly the view `new` mapped.
        unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.ptr }) };
    }
}

impl FrameReader {
    /// Creates (or truncates) the frame file at `path`.
    pub fn create(path: &Path) -> io::Result<FrameReader> {
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(true);
        // Windows: keep the frames in memory instead of writing them to the disk (FILE_ATTRIBUTE_TEMPORARY).
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::attributes(&mut opts, 0x100);
        let file = opts.open(path)?;
        file.set_len(HEADER + SLOTS * MAX_FRAME_BYTES)?;
        let mut h = [0u8; HEADER as usize];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&1u32.to_le_bytes());
        h[8..16].copy_from_slice(&MAX_FRAME_BYTES.to_le_bytes());
        write_at(&file, &h, 0)?;
        Ok(FrameReader {
            file,
            path: path.to_path_buf(),
            cap: MAX_FRAME_BYTES,
            seen: 0,
            #[cfg(any(unix, windows))]
            map: None,
            #[cfg(not(any(unix, windows)))]
            scratch: Vec::new(),
        })
    }

    /// Sequence number of the newest frame (changes whenever the runner published one). Cheap: reads 8 bytes.
    pub fn seq(&self) -> io::Result<u64> {
        let mut b = [0u8; 8];
        read_at(&self.file, &mut b, 16)?;
        Ok(u64::from_le_bytes(b))
    }

    /// Copies the newest frame into `buf` (resized to fit) unless it was already returned. `None` when there is no
    /// new frame, or when the runner overwrote it while it was read (the next call gets the newer one).
    pub fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<Option<FrameInfo>> {
        let Some((info, at, len)) = self.newest()? else { return Ok(None) };
        buf.resize(len as usize, 0);
        read_at(&self.file, buf, at)?;
        self.accept(info)
    }

    /// Like [`FrameReader::read`], but hands the newest frame's pixels to `use_pixels` where they are (a mapping of
    /// the file) instead of copying them out first. `use_pixels` must be done with them when it returns; when the
    /// runner overwrote the frame meanwhile the result is `None` (and whatever `use_pixels` made of it is stale).
    pub fn read_with(&mut self, mut use_pixels: impl FnMut(FrameInfo, &[u8])) -> io::Result<Option<FrameInfo>> {
        self.read_with_dyn(&mut use_pixels)
    }

    fn read_with_dyn(&mut self, use_pixels: &mut dyn FnMut(FrameInfo, &[u8])) -> io::Result<Option<FrameInfo>> {
        let Some((info, at, len)) = self.newest()? else { return Ok(None) };
        #[cfg(any(unix, windows))]
        {
            if self.map.is_none() {
                self.map = Some(Map::new(&self.file, HEADER + SLOTS * self.cap)?);
            }
            use_pixels(info, self.map.as_ref().expect("mapped above").bytes(at, len));
        }
        #[cfg(not(any(unix, windows)))]
        {
            let mut buf = std::mem::take(&mut self.scratch);
            buf.resize(len as usize, 0);
            read_at(&self.file, &mut buf, at)?;
            use_pixels(info, &buf);
            self.scratch = buf;
        }
        self.accept(info)
    }

    /// The newest frame when it is new: its description, where its pixels are and how many bytes they take.
    fn newest(&self) -> io::Result<Option<(FrameInfo, u64, u64)>> {
        let h = read_header(&self.file)?;
        if h.seq == 0 || h.seq == self.seen {
            return Ok(None);
        }
        let len = u64::from(h.width) * u64::from(h.height) * 4;
        let layout = Layout::from_code(h.layout);
        let (Some(layout), true) = (layout, len != 0 && len <= self.cap && u64::from(h.slot) < SLOTS) else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame header"));
        };
        let info = FrameInfo { width: h.width, height: h.height, seq: h.seq, layout };
        Ok(Some((info, slot_offset(u64::from(h.slot), self.cap), len)))
    }

    /// `info` once its pixels have been taken, unless the runner has come round to its slot meanwhile.
    fn accept(&mut self, info: FrameInfo) -> io::Result<Option<FrameInfo>> {
        let intact = !overwritten(info.seq, self.seq()?);
        Ok(intact.then(|| {
            self.seen = info.seq;
            info
        }))
    }

    /// Deletes the file (the runner keeps its handle until it exits).
    pub fn remove(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The runner's end.
pub struct FrameWriter {
    file: File,
    cap: u64,
    seq: u64,
    /// Windows: the pixels go through a mapping of the file (the header still through `WriteFile`, after them: both
    /// see the same cached pages).
    #[cfg(windows)]
    map: Option<Map>,
}

impl FrameWriter {
    /// Opens a file made by [`FrameReader::create`].
    pub fn open(path: &Path) -> io::Result<FrameWriter> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let h = read_header(&file)?;
        #[cfg(windows)]
        let map = Map::new_rw(&file, HEADER + SLOTS * h.cap).ok();
        Ok(FrameWriter {
            file,
            cap: h.cap,
            seq: h.seq,
            #[cfg(windows)]
            map,
        })
    }

    /// Publishes a `width` x `height` frame of BGRA pixels. `Ok(false)` when it does not fit the file.
    pub fn publish(&mut self, width: u32, height: u32, bgra: &[u8]) -> io::Result<bool> {
        self.publish_as(width, height, Layout::Bgra, bgra)
    }

    /// Publishes a `width` x `height` frame of pixels in `layout`. `Ok(false)` when it does not fit the file.
    pub fn publish_as(&mut self, width: u32, height: u32, layout: Layout, pixels: &[u8]) -> io::Result<bool> {
        let len = u64::from(width) * u64::from(height) * 4;
        if len == 0 || len > self.cap || pixels.len() as u64 != len {
            return Ok(false);
        }
        let next = self.seq + 1;
        let slot = next % SLOTS;
        #[cfg(windows)]
        match self.map.as_mut() {
            Some(m) => m.write(slot_offset(slot, self.cap), pixels),
            None => write_at(&self.file, pixels, slot_offset(slot, self.cap))?,
        }
        #[cfg(not(windows))]
        write_at(&self.file, pixels, slot_offset(slot, self.cap))?;
        let mut h = [0u8; 16];
        h[0..4].copy_from_slice(&width.to_le_bytes());
        h[4..8].copy_from_slice(&height.to_le_bytes());
        h[8..12].copy_from_slice(&(slot as u32).to_le_bytes());
        h[12..16].copy_from_slice(&layout.code().to_le_bytes());
        write_at(&self.file, &h, 24)?;
        write_at(&self.file, &next.to_le_bytes(), 16)?;
        self.seq = next;
        Ok(true)
    }
}

/// Modifier bits of [`Cmd::Button`], [`Cmd::Wheel`] and [`Cmd::Key`].
/// Runner -> launcher (stdout): the game copied text. The in-window game has no clipboard of its own (SDL's offscreen
/// driver), so the launcher puts it on the desktop clipboard. `%`, CR, LF and other control characters are
/// `%XX`-escaped so the text stays on one line.
pub const COPY_PREFIX: &str = "@copy ";

/// Runner -> launcher (stdout): the game shows an HTML page (the server's welcome message, help). The in-window game
/// has the launcher show it in place of the game, as the phone shows its web view; escaped like [`COPY_PREFIX`].
pub const PAGE_PREFIX: &str = "@page ";

/// `prefix` and `text` on one line: `%` and control characters `%XX`-escaped.
fn escaped_line(prefix: &str, text: &str) -> String {
    let mut out = String::from(prefix);
    for c in text.chars() {
        if c == '%' || c.is_control() {
            let mut b = [0u8; 4];
            for byte in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The text of an [`escaped_line`] with this prefix (`None` for any other line).
fn parse_escaped(prefix: &str, line: &str) -> Option<String> {
    let rest = line.trim_end_matches(['\r', '\n']).strip_prefix(prefix)?.as_bytes();
    let mut out = Vec::with_capacity(rest.len());
    let mut i = 0;
    while i < rest.len() {
        let hex = rest
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (rest[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The `@copy` line for `text`.
pub fn copy_line(text: &str) -> String {
    escaped_line(COPY_PREFIX, text)
}

/// The text of an `@copy` line (`None` for any other line).
pub fn parse_copy(line: &str) -> Option<String> {
    parse_escaped(COPY_PREFIX, line)
}

/// The `@page` line for a self-contained HTML page.
pub fn page_line(html: &str) -> String {
    escaped_line(PAGE_PREFIX, html)
}

/// The HTML of an `@page` line (`None` for any other line).
pub fn parse_page(line: &str) -> Option<String> {
    parse_escaped(PAGE_PREFIX, line)
}

pub const MOD_CTRL: u8 = 1;
pub const MOD_SHIFT: u8 = 2;
pub const MOD_ALT: u8 = 4;

/// An input or window command for the runner.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// The widget is now this big (device pixels): the runner renders frames of this size.
    Size {
        width: u32,
        height: u32,
    },
    Move {
        x: f32,
        y: f32,
    },
    /// Button 0 left, 1 middle, 2 right, others by number.
    Button {
        button: u8,
        pressed: bool,
        x: f32,
        y: f32,
        mods: u8,
    },
    Wheel {
        dx: f32,
        dy: f32,
        x: f32,
        y: f32,
        mods: u8,
    },
    /// `hid` is a USB HID usage id (SDL scancode).
    Key {
        hid: u32,
        pressed: bool,
        repeat: bool,
        mods: u8,
    },
    Text(String),
    Focus(bool),
    Visible(bool),
    /// The pixel layout the view wants frames in from now on.
    Layout(Layout),
    /// Developer mode: stop the session's mods and load them again from their folders.
    ModsReload,
    /// Say a line in the game's chat as if the player had typed it (a `/command` of a mod runs). For tests and
    /// developer tools: the game's chat field is not reachable from outside the window.
    ModsChat(String),
    Quit,
}

fn flag(b: bool) -> u8 {
    u8::from(b)
}

impl Cmd {
    /// The command as one line (no newline).
    pub fn encode(&self) -> String {
        match self {
            Cmd::Size { width, height } => format!("size {width} {height}"),
            Cmd::Move { x, y } => format!("move {x} {y}"),
            Cmd::Button { button, pressed, x, y, mods } => format!("button {button} {} {x} {y} {mods}", flag(*pressed)),
            Cmd::Wheel { dx, dy, x, y, mods } => format!("wheel {dx} {dy} {x} {y} {mods}"),
            Cmd::Key { hid, pressed, repeat, mods } => format!("key {hid} {} {} {mods}", flag(*pressed), flag(*repeat)),
            Cmd::Text(t) => format!("text {}", escape(t)),
            Cmd::Focus(on) => format!("focus {}", flag(*on)),
            Cmd::Visible(on) => format!("visible {}", flag(*on)),
            Cmd::Layout(l) => format!("layout {}", l.code()),
            Cmd::ModsReload => "mods-reload".to_string(),
            Cmd::ModsChat(t) => format!("mods-chat {}", escape(t)),
            Cmd::Quit => "quit".to_string(),
        }
    }

    /// Parses a line made by [`Cmd::encode`]; `None` for anything else.
    pub fn parse(line: &str) -> Option<Cmd> {
        let line = line.trim_end_matches(['\n', '\r']);
        let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
        if name == "text" {
            return unescape(rest).map(Cmd::Text);
        }
        if name == "mods-chat" {
            return unescape(rest).map(Cmd::ModsChat);
        }
        let mut it = rest.split(' ');
        let mut num = || it.next();
        let on = |s: Option<&str>| match s {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        };
        Some(match name {
            "size" => Cmd::Size { width: num()?.parse().ok()?, height: num()?.parse().ok()? },
            "move" => Cmd::Move { x: num()?.parse().ok()?, y: num()?.parse().ok()? },
            "button" => Cmd::Button {
                button: num()?.parse().ok()?,
                pressed: on(num())?,
                x: num()?.parse().ok()?,
                y: num()?.parse().ok()?,
                mods: num()?.parse().ok()?,
            },
            "wheel" => Cmd::Wheel {
                dx: num()?.parse().ok()?,
                dy: num()?.parse().ok()?,
                x: num()?.parse().ok()?,
                y: num()?.parse().ok()?,
                mods: num()?.parse().ok()?,
            },
            "key" => Cmd::Key {
                hid: num()?.parse().ok()?,
                pressed: on(num())?,
                repeat: on(num())?,
                mods: num()?.parse().ok()?,
            },
            "focus" => Cmd::Focus(on(num())?),
            "visible" => Cmd::Visible(on(num())?),
            "layout" => Cmd::Layout(Layout::from_code(num()?.parse().ok()?)?),
            "mods-reload" => Cmd::ModsReload,
            "quit" => Cmd::Quit,
            _ => return None,
        })
    }
}

/// A text on one line: backslash, newline and carriage return are escaped.
fn escape(t: &str) -> String {
    t.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r")
}

/// The text of [`escape`]; `None` for an escape that does not exist.
fn unescape(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            match chars.next()? {
                'n' => '\n',
                'r' => '\r',
                '\\' => '\\',
                _ => return None,
            }
        } else {
            c
        });
    }
    Some(out)
}

/// USB HID usage id of a PC keyboard scan code (set 1) as Windows reports it (`WM_KEYDOWN` lParam bits 16-23, with the
/// extended-key flag, bit 24, for the `E0`-prefixed keys). Linux evdev codes are set 1 for the main block, so this
/// only moves the extended keys to their evdev codes. `None` for keys the game has no use for.
pub fn hid_from_scancode(code: u32, extended: bool) -> Option<u32> {
    const EXTENDED: [(u32, u32); 17] = [
        (0x1c, 96),
        (0x1d, 97),
        (0x35, 98),
        (0x38, 100),
        (0x47, 102),
        (0x48, 103),
        (0x49, 104),
        (0x4b, 105),
        (0x4d, 106),
        (0x4f, 107),
        (0x50, 108),
        (0x51, 109),
        (0x52, 110),
        (0x53, 111),
        (0x5b, 125),
        (0x5c, 126),
        (0x5d, 127),
    ];
    if !extended {
        return hid_from_evdev(code);
    }
    EXTENDED.iter().find(|&&(sc, _)| sc == code).and_then(|&(_, evdev)| hid_from_evdev(evdev))
}

/// USB HID usage id of a macOS virtual key code (`-[NSEvent keyCode]`, the `kVK_*` constants: the key's position on
/// an ANSI keyboard, whatever the layout). Mapped through the evdev code of the same key, so the set of keys is the
/// same as on Linux. `None` for keys the game has no use for.
pub fn hid_from_mac_keycode(code: u16) -> Option<u32> {
    const EVDEV: [(u16, u32); 64] = [
        (0x00, 30),  // A
        (0x01, 31),  // S
        (0x02, 32),  // D
        (0x03, 33),  // F
        (0x04, 35),  // H
        (0x05, 34),  // G
        (0x06, 44),  // Z
        (0x07, 45),  // X
        (0x08, 46),  // C
        (0x09, 47),  // V
        (0x0B, 48),  // B
        (0x0C, 16),  // Q
        (0x0D, 17),  // W
        (0x0E, 18),  // E
        (0x0F, 19),  // R
        (0x10, 21),  // Y
        (0x11, 20),  // T
        (0x12, 2),   // 1
        (0x13, 3),   // 2
        (0x14, 4),   // 3
        (0x15, 5),   // 4
        (0x16, 7),   // 6
        (0x17, 6),   // 5
        (0x18, 13),  // =
        (0x19, 10),  // 9
        (0x1A, 8),   // 7
        (0x1B, 12),  // -
        (0x1C, 9),   // 8
        (0x1D, 11),  // 0
        (0x1E, 27),  // ]
        (0x1F, 24),  // O
        (0x20, 22),  // U
        (0x21, 26),  // [
        (0x22, 23),  // I
        (0x23, 25),  // P
        (0x24, 28),  // Return
        (0x25, 38),  // L
        (0x26, 36),  // J
        (0x27, 40),  // '
        (0x28, 37),  // K
        (0x29, 39),  // ;
        (0x2A, 43),  // backslash
        (0x2B, 51),  // ,
        (0x2C, 53),  // /
        (0x2D, 49),  // N
        (0x2E, 50),  // M
        (0x2F, 52),  // .
        (0x30, 15),  // Tab
        (0x31, 57),  // Space
        (0x32, 41),  // `
        (0x33, 14),  // Delete (backspace)
        (0x35, 1),   // Escape
        (0x38, 42),  // Shift
        (0x39, 58),  // Caps Lock
        (0x3A, 56),  // Option
        (0x3B, 29),  // Control
        (0x3C, 54),  // right Shift
        (0x3D, 100), // right Option
        (0x3E, 97),  // right Control
        (0x73, 102), // Home
        (0x74, 104), // Page Up
        (0x75, 111), // Forward Delete
        (0x77, 107), // End
        (0x79, 109), // Page Down
    ];
    // F1..F12 and the arrows.
    const MORE: [(u16, u32); 16] = [
        (0x7A, 59),
        (0x78, 60),
        (0x63, 61),
        (0x76, 62),
        (0x60, 63),
        (0x61, 64),
        (0x62, 65),
        (0x64, 66),
        (0x65, 67),
        (0x6D, 68),
        (0x67, 87),
        (0x6F, 88),
        (0x7B, 105),
        (0x7C, 106),
        (0x7D, 108),
        (0x7E, 103),
    ];
    EVDEV.iter().chain(MORE.iter()).find(|&&(k, _)| k == code).and_then(|&(_, evdev)| hid_from_evdev(evdev))
}

/// USB HID usage id of a Linux evdev key code (GDK's hardware keycode minus 8 on X11 and Wayland); the physical
/// key, like an SDL scancode. `None` for keys the game has no use for.
pub fn hid_from_evdev(code: u32) -> Option<u32> {
    const ROW_Q: [u32; 12] = [20, 26, 8, 21, 23, 28, 24, 12, 18, 19, 47, 48];
    const ROW_A: [u32; 11] = [4, 22, 7, 9, 10, 11, 13, 14, 15, 51, 52];
    const ROW_Z: [u32; 7] = [29, 27, 6, 25, 5, 17, 16];
    Some(match code {
        1 => 41,
        2..=10 => 30 + (code - 2),
        11 => 39,
        12 => 45,
        13 => 46,
        14 => 42,
        15 => 43,
        16..=27 => ROW_Q[(code - 16) as usize],
        28 => 40,
        29 => 224,
        30..=40 => ROW_A[(code - 30) as usize],
        41 => 53,
        42 => 225,
        43 => 49,
        44..=50 => ROW_Z[(code - 44) as usize],
        51 => 54,
        52 => 55,
        53 => 56,
        54 => 229,
        56 => 226,
        57 => 44,
        58 => 57,
        59..=68 => 58 + (code - 59),
        87 => 68,
        88 => 69,
        97 => 228,
        100 => 230,
        102 => 74,
        103 => 82,
        104 => 75,
        105 => 80,
        106 => 79,
        107 => 77,
        108 => 81,
        109 => 78,
        110 => 73,
        111 => 76,
        _ => return None,
    })
}
