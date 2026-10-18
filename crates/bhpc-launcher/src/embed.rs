//! Playing inside the launcher window: how the game runner (`bhpc-run --embed FILE`) and the launcher talk.
//!
//! * Frames go through a **frame file** in shared memory (`/dev/shm` or the runtime dir; any file works). The
//!   launcher creates it ([`FrameReader::create`]), the runner fills it ([`FrameWriter`]), and the launcher paints the
//!   newest frame into a native widget. The file has a small header and three pixel slots; the writer never touches
//!   the slot the header currently names, so the reader gets whole frames without locks (a reader that is two
//!   frames late detects that and skips). Pixels are BGRA, top row first, premultiplied (opaque), which is what
//!   cairo and most compositors take.
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
fn write_at(f: &File, buf: &[u8], at: u64) -> io::Result<()> {
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
fn write_at(f: &File, mut buf: &[u8], mut at: u64) -> io::Result<()> {
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

/// Header: magic, version, slot capacity, sequence number of the newest frame (0 = none), its width, height, slot.
struct Header {
    cap: u64,
    seq: u64,
    width: u32,
    height: u32,
    slot: u32,
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
}

/// The launcher's end: creates the file and reads the newest frame.
pub struct FrameReader {
    file: File,
    path: PathBuf,
    cap: u64,
    seen: u64,
}

impl FrameReader {
    /// Creates (or truncates) the frame file at `path`.
    pub fn create(path: &Path) -> io::Result<FrameReader> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
        file.set_len(HEADER + SLOTS * MAX_FRAME_BYTES)?;
        let mut h = [0u8; HEADER as usize];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&1u32.to_le_bytes());
        h[8..16].copy_from_slice(&MAX_FRAME_BYTES.to_le_bytes());
        write_at(&file, &h, 0)?;
        Ok(FrameReader { file, path: path.to_path_buf(), cap: MAX_FRAME_BYTES, seen: 0 })
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
        let h = read_header(&self.file)?;
        if h.seq == 0 || h.seq == self.seen {
            return Ok(None);
        }
        let len = u64::from(h.width) * u64::from(h.height) * 4;
        if len == 0 || len > self.cap || u64::from(h.slot) >= SLOTS {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame header"));
        }
        buf.resize(len as usize, 0);
        read_at(&self.file, buf, slot_offset(u64::from(h.slot), self.cap))?;
        let intact = !overwritten(h.seq, self.seq()?);
        Ok(intact.then(|| {
            self.seen = h.seq;
            FrameInfo { width: h.width, height: h.height, seq: h.seq }
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
}

impl FrameWriter {
    /// Opens a file made by [`FrameReader::create`].
    pub fn open(path: &Path) -> io::Result<FrameWriter> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let h = read_header(&file)?;
        Ok(FrameWriter { file, cap: h.cap, seq: h.seq })
    }

    /// Publishes a `width` x `height` frame of BGRA pixels. `Ok(false)` when it does not fit the file.
    pub fn publish(&mut self, width: u32, height: u32, bgra: &[u8]) -> io::Result<bool> {
        let len = u64::from(width) * u64::from(height) * 4;
        if len == 0 || len > self.cap || bgra.len() as u64 != len {
            return Ok(false);
        }
        let next = self.seq + 1;
        let slot = next % SLOTS;
        write_at(&self.file, bgra, slot_offset(slot, self.cap))?;
        let mut h = [0u8; 20];
        h[0..4].copy_from_slice(&width.to_le_bytes());
        h[4..8].copy_from_slice(&height.to_le_bytes());
        h[8..12].copy_from_slice(&(slot as u32).to_le_bytes());
        write_at(&self.file, &h[..12], 24)?;
        write_at(&self.file, &next.to_le_bytes(), 16)?;
        self.seq = next;
        Ok(true)
    }
}

/// Modifier bits of [`Cmd::Button`], [`Cmd::Wheel`] and [`Cmd::Key`].
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
            Cmd::Text(t) => format!("text {}", t.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r")),
            Cmd::Focus(on) => format!("focus {}", flag(*on)),
            Cmd::Visible(on) => format!("visible {}", flag(*on)),
            Cmd::Quit => "quit".to_string(),
        }
    }

    /// Parses a line made by [`Cmd::encode`]; `None` for anything else.
    pub fn parse(line: &str) -> Option<Cmd> {
        let line = line.trim_end_matches(['\n', '\r']);
        let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
        if name == "text" {
            let mut out = String::new();
            let mut chars = rest.chars();
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
            return Some(Cmd::Text(out));
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
            "quit" => Cmd::Quit,
            _ => return None,
        })
    }
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
