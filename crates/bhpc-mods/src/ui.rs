//! The in-game mod UI, drawn by the loader itself in the IG pixel kit (notes/design-source IG-Spec, IG-Kit,
//! IG-Join, IG-Mods, IG-HUD): the join prompt ("this server uses mods", downloading, failures, the server's
//! rejection text), the Mods panel (F9: running mods, budget state, time used, the log) and toasts.
//!
//! [`ModUi::render`] paints a premultiplied RGBA picture of the window's size that the window blends over the game
//! (the same path as the game's popups), so it works in the launcher's embedded view and in the SDL window, and
//! shows in screenshots. [`ModUi::input`] takes the window's keys and clicks while the UI is modal. The unit `u` is
//! 3 px at 1280x720 and scales with the window (at least 2 px), as IG-Spec asks.

use std::sync::{Arc, OnceLock};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

use crate::view::{JoinStage, JoinView, UiCmd, View};

static TITLE_FONT: &[u8] = include_bytes!("../fonts/Tiny5-Regular.ttf");
static BODY_FONT: &[u8] = include_bytes!("../../bhpc-vfs/fonts/LiberationSans-Regular.ttf");
static BOLD_FONT: &[u8] = include_bytes!("../../bhpc-vfs/fonts/LiberationSans-Bold.ttf");

fn fonts() -> &'static [FontRef<'static>; 3] {
    static F: OnceLock<[FontRef<'static>; 3]> = OnceLock::new();
    F.get_or_init(|| {
        let f = |b: &'static [u8]| FontRef::try_from_slice(b).expect("bundled font parses");
        [f(TITLE_FONT), f(BODY_FONT), f(BOLD_FONT)]
    })
}

/// Which typeface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    /// Tiny5 (titles, buttons).
    Pixel,
    Body,
    Bold,
}

type Rgb = u32;

const INK: Rgb = 0x17110D;
const PANEL: [Rgb; 3] = [0x5E4A3C, 0x3D3029, 0x2A201B];
const BANNER: [Rgb; 3] = [0xE67A45, 0xBF4F1C, 0x8A3412];
const SLOT: [Rgb; 3] = [0x544235, 0x342923, 0x241C18];
const WELL: [Rgb; 3] = [0x1A1410, 0x2A221D, 0x4A3B31];
const FIELD: [Rgb; 3] = [0x0F0B09, 0x1F1814, 0x3A2E26];
const N_DAN: [Rgb; 3] = [0xE0533D, 0x45160F, 0x2F0E09];
const N_INFO: [Rgb; 3] = [0x4F7AB0, 0x1C3048, 0x142338];
const BTN: [Rgb; 4] = [0x7A6553, 0x5E4A3C, 0x4A3A2F, 0x241B16];
const BTN_PRI: [Rgb; 4] = [0xFFEBB8, 0xFFD27A, 0xE8B456, 0xB97A22];
const BTN_DIS: [Rgb; 4] = [0x2A221D, 0x3A302A, 0x3A302A, 0x2A221D];
const TEXT: Rgb = 0xFBF3E6;
const TEXT2: Rgb = 0xDCCDBB;
const TEXT3: Rgb = 0xBBA894;
const TEXT_DIS: Rgb = 0x9C8E82;
const INK_ON_SUN: Rgb = 0x1F1A17;
const SUN: Rgb = 0xFFD27A;
const DANGER: Rgb = 0xD23B26;
const TILES: [[Rgb; 3]; 6] = [
    [0x82B694, 0x3E8E5A, 0x2B633F],
    [0xB394D7, 0x8A5BC2, 0x614088],
    [0x78ACD1, 0x2F7FB8, 0x215981],
    [0xD1B278, 0xB8892F, 0x816021],
    [0x78B6B6, 0x2F8F8F, 0x216464],
    [0xBD789E, 0x9A2F6A, 0x6C214A],
];
/// Badge kinds: `(background, text, border)`.
const B_DEFAULT: [Rgb; 3] = [0x2A221D, 0xDCCDBB, 0x54463B];
const B_OK: [Rgb; 3] = [0x163526, 0x8FE5B4, 0x2F7A50];
const B_BAD: [Rgb; 3] = [0x45160F, 0xFFB4A6, 0xA8321F];
const B_WARN: [Rgb; 3] = [0x43350F, 0xFFDE85, 0x8F6F1C];
const B_INFO: [Rgb; 3] = [0x1C3048, 0xA9CCFF, 0x3F6699];
const B_NATIVE: [Rgb; 3] = [0xB02E1C, 0xFFF6EE, INK];

/// A premultiplied RGBA canvas.
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w: w as i32, h: h as i32, px: vec![0; w as usize * h as usize * 4] }
    }

    fn blend(&mut self, x: i32, y: i32, rgb: Rgb, a: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let i = (y as usize * self.w as usize + x as usize) * 4;
        let a = a.min(1.0);
        let src = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
        for (d, s) in self.px[i..i + 3].iter_mut().zip(src) {
            *d = (s as f32 * a + *d as f32 * (1.0 - a)).round() as u8;
        }
        let d = self.px[i + 3] as f32;
        self.px[i + 3] = (255.0 * a + d * (1.0 - a)).round() as u8;
    }

    /// Fills `[x, x+w) x [y, y+h)` with `rgb` at opacity `a`.
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: Rgb, a: f32) {
        for yy in y.max(0)..(y + h).min(self.h) {
            for xx in x.max(0)..(x + w).min(self.w) {
                self.blend(xx, yy, rgb, a);
            }
        }
    }

    /// The kit's 9-slice: a 1u ink outline with cut corners, 1u highlight (top, left), 1u shade (bottom, right).
    fn bevel(&mut self, r: Rect, u: i32, c: [Rgb; 3], ink: Rgb) {
        let Rect { x, y, w, h } = r;
        self.fill(x + u, y, w - 2 * u, u, ink, 1.0);
        self.fill(x + u, y + h - u, w - 2 * u, u, ink, 1.0);
        self.fill(x, y + u, u, h - 2 * u, ink, 1.0);
        self.fill(x + w - u, y + u, u, h - 2 * u, ink, 1.0);
        self.fill(x + u, y + u, w - 2 * u, h - 2 * u, c[1], 1.0);
        self.fill(x + u, y + u, w - 2 * u, u, c[0], 1.0);
        self.fill(x + u, y + u, u, h - 2 * u, c[0], 1.0);
        self.fill(x + u, y + h - 2 * u, w - 2 * u, u, c[2], 1.0);
        self.fill(x + w - 2 * u, y + 2 * u, u, h - 3 * u, c[2], 1.0);
    }

    /// Draws `text` with its top-left at `(x, y)`; returns the advance width.
    pub fn text(&mut self, face: Face, size: f32, x: f32, y: f32, rgb: Rgb, text: &str) -> f32 {
        let font = &fonts()[face as usize];
        let sf = font.as_scaled(PxScale::from(size));
        let mut pen = x;
        let base = y + sf.ascent();
        let mut prev = None;
        for ch in text.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                pen += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, base));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| self.blend(b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, rgb, cov));
            }
            pen += sf.h_advance(id);
            prev = Some(id);
        }
        pen - x
    }

    /// Text with a hard shadow `off` pixels down and right: `colors` is `(text, shadow)`.
    fn shadow_text(&mut self, face: Face, size: f32, at: (f32, f32), colors: (Rgb, Rgb), off: f32, t: &str) {
        self.text(face, size, at.0 + off, at.1 + off, colors.1, t);
        self.text(face, size, at.0, at.1, colors.0, t);
    }
}

/// Width of `text` at `size`.
pub fn measure(face: Face, size: f32, text: &str) -> f32 {
    let font = &fonts()[face as usize];
    let sf = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// `text` split into lines no wider than `width`.
pub fn wrap(face: Face, size: f32, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && measure(face, size, &cand) > width {
                lines.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = cand;
            }
        }
        lines.push(line);
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && y >= self.y as f32 && x < (self.x + self.w) as f32 && y < (self.y + self.h) as f32
    }
}

/// Something that can be clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Accept,
    Decline,
    Cancel,
    Dismiss,
    NativeBox,
    ClosePanel,
}

/// Window input the UI may take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiInput {
    /// A key press (USB HID usage id).
    Key(u32),
    /// A left click in window pixels (the picture's coordinates).
    Click(f32, f32),
    /// Anything else the window sends (mouse moves, releases, text, wheel).
    Other,
}

pub const HID_F9: u32 = 66;
pub const HID_ENTER: u32 = 40;
pub const HID_ESCAPE: u32 = 41;
pub const HID_SPACE: u32 = 44;
pub const HID_F5: u32 = 62;

/// Byte sizes for people.
pub fn size_text(b: u64) -> String {
    match b {
        0..1024 => format!("{b} B"),
        1024..1_048_576 => format!("{} KB", b.div_ceil(1024)),
        _ => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

/// The last picture: view serial, size, panel open, native box ticked, pixels.
type Drawn = (u64, (u32, u32), bool, bool, Arc<Vec<u8>>);

/// The mod UI's own state (what is open, the native confirmation, the clickable areas of the last picture).
#[derive(Default)]
pub struct ModUi {
    pub panel: bool,
    pub native_ok: bool,
    hits: Vec<(Rect, Hit)>,
    last: Option<Drawn>,
}

impl ModUi {
    pub fn new() -> ModUi {
        ModUi::default()
    }

    /// True while the UI takes the window's input.
    pub fn modal(&self, v: &View) -> bool {
        v.join.is_some() || self.panel
    }

    fn act(&mut self, h: Hit, v: &View) -> Option<UiCmd> {
        match h {
            Hit::Accept => {
                let ok = v.join.as_ref().is_some_and(|j| j.can_accept(self.native_ok));
                ok.then_some(UiCmd::Decide { accept: true, native_ok: self.native_ok })
            }
            Hit::Decline => Some(UiCmd::Decide { accept: false, native_ok: false }),
            Hit::Cancel => Some(UiCmd::Cancel),
            Hit::Dismiss => Some(UiCmd::Dismiss),
            Hit::NativeBox => {
                self.native_ok = !self.native_ok;
                None
            }
            Hit::ClosePanel => {
                self.panel = false;
                None
            }
        }
    }

    /// Handles one input. Returns whether the UI took it (the game must not see it) and what the player decided.
    pub fn input(&mut self, ev: UiInput, v: &View) -> (bool, Option<UiCmd>) {
        if let Some(j) = &v.join {
            let key = |k: u32| -> Option<Hit> {
                match (&j.stage, k) {
                    (JoinStage::Prompt, HID_ENTER) => Some(Hit::Accept),
                    (JoinStage::Prompt, HID_ESCAPE) => Some(Hit::Decline),
                    (JoinStage::Prompt, HID_SPACE) => Some(Hit::NativeBox),
                    (JoinStage::Downloading, HID_ESCAPE) => Some(Hit::Cancel),
                    (JoinStage::Failed(_) | JoinStage::Rejected(..), HID_ENTER | HID_ESCAPE) => Some(Hit::Dismiss),
                    _ => None,
                }
            };
            let hit = match ev {
                UiInput::Key(k) => key(k),
                UiInput::Click(x, y) => self.hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h),
                UiInput::Other => None,
            };
            return (true, hit.and_then(|h| self.act(h, v)));
        }
        match ev {
            UiInput::Key(HID_F9) => {
                self.panel = !self.panel;
                (true, None)
            }
            UiInput::Key(HID_F5) if v.dev => (true, Some(UiCmd::Reload)),
            _ if !self.panel => (false, None),
            UiInput::Key(HID_ESCAPE) => {
                self.panel = false;
                (true, None)
            }
            UiInput::Click(x, y) => {
                let hit = self.hits.iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h);
                (true, hit.and_then(|h| self.act(h, v)))
            }
            _ => (true, None),
        }
    }

    /// The picture for `v` at `(w, h)` window pixels; `None` when nothing shows. `serial` identifies the view's
    /// content (the picture is reused while it and the UI state stay the same).
    pub fn render(&mut self, v: &View, serial: u64, w: u32, h: u32) -> Option<Arc<Vec<u8>>> {
        if v.join.is_none() && !self.panel && v.toasts.is_empty() {
            self.hits.clear();
            self.last = None;
            return None;
        }
        if let Some((s, size, p, n, px)) = &self.last {
            if *s == serial && *size == (w, h) && *p == self.panel && *n == self.native_ok {
                return Some(px.clone());
            }
        }
        let mut c = Canvas::new(w, h);
        self.hits.clear();
        let u = ((3.0 * (w as f32 / 1280.0).min(h as f32 / 720.0)).round() as i32).max(2);
        if let Some(j) = &v.join {
            self.join(&mut c, u, j);
        } else if self.panel {
            self.mods_panel(&mut c, u, v);
        }
        toasts(&mut c, u, v);
        let px = Arc::new(c.px);
        self.last = Some((serial, (w, h), self.panel, self.native_ok, px.clone()));
        Some(px)
    }

    /// A button whose top-right corner is `at` (`(right, top)`); returns its left edge.
    fn button(&mut self, c: &mut Canvas, u: i32, at: (i32, i32), label: &str, kind: [Rgb; 4], hit: Hit) -> i32 {
        let (right, y) = at;
        let size = 8.0 * u as f32;
        let tw = measure(Face::Pixel, size, label) as i32;
        let (bw, bh) = (tw + 10 * u, 18 * u);
        let r = Rect { x: right - bw, y, w: bw, h: bh };
        c.bevel(Rect { h: bh - 2 * u, ..r }, u, [kind[0], kind[1], kind[2]], INK);
        c.fill(r.x + u, r.y + bh - 3 * u, bw - 2 * u, 2 * u, kind[3], 1.0);
        c.fill(r.x + u, r.y + bh - u, bw - 2 * u, u, INK, 1.0);
        let fg = match kind {
            BTN_PRI => INK_ON_SUN,
            BTN_DIS => TEXT_DIS,
            _ => TEXT,
        };
        c.text(Face::Pixel, size, (r.x + 5 * u) as f32, (r.y + 4 * u) as f32, fg, label);
        self.hits.push((r, hit));
        r.x
    }

    fn badge(c: &mut Canvas, u: i32, x: i32, y: i32, text: &str, kind: [Rgb; 3]) -> i32 {
        let size = 5.5 * u as f32;
        let w = measure(Face::Bold, size, text) as i32 + 5 * u;
        let h = 9 * u;
        c.fill(x, y, w, h, kind[2], 1.0);
        c.fill(x + u / 2, y + u / 2, w - u, h - u, kind[0], 1.0);
        if kind == B_NATIVE {
            for i in (0..w).step_by((4 * u) as usize) {
                c.fill(x + i, y + u / 2, 2 * u, h - u, 0x8E2516, 0.6);
            }
        }
        c.text(Face::Bold, size, (x + 5 * u / 2) as f32, (y + 3 * u / 2) as f32, kind[1], text);
        x + w + 2 * u
    }

    /// A dialog frame: dimmed scene, panel with a banner title; returns the inner content rectangle.
    fn dialog(c: &mut Canvas, u: i32, width: i32, height: i32, title: &str) -> Rect {
        c.fill(0, 0, c.w, c.h, 0x0B0806, 0.89);
        let w = width.min(c.w - 8 * u);
        let h = height.min(c.h - 8 * u);
        let (x, y) = ((c.w - w) / 2, (c.h - h) / 2);
        c.bevel(Rect { x, y, w, h }, u, PANEL, INK);
        let bh = 17 * u;
        c.bevel(Rect { x, y, w, h: bh }, u, BANNER, INK);
        c.shadow_text(
            Face::Pixel,
            10.5 * u as f32,
            ((x + 5 * u) as f32, (y + 4 * u) as f32),
            (0xFFF6EA, 0x5A200A),
            u as f32,
            title,
        );
        Rect { x: x + 5 * u, y: y + bh + 4 * u, w: w - 10 * u, h: h - bh - 4 * u }
    }

    fn join(&mut self, c: &mut Canvas, u: i32, j: &JoinView) {
        let fs = 6.3 * u as f32;
        let small = 5.3 * u as f32;
        let n = j.mods.len() as i32;
        let native = j.mods.iter().any(|m| m.native);
        let row_h = 24 * u;
        let (title, height) = match &j.stage {
            JoinStage::Prompt => {
                let m = if n == 1 { "mod".to_string() } else { "mods".to_string() };
                (
                    format!("{} uses {n} {m}", j.server),
                    17 * u + 4 * u + 16 * u + n * (row_h + 3 * u) + if native { 44 * u } else { 0 } + 12 * u + 26 * u,
                )
            }
            JoinStage::Downloading => {
                (format!("Joining {}", j.server), 17 * u + 4 * u + 26 * u + n * (row_h + 3 * u) + 12 * u + 26 * u)
            }
            JoinStage::Failed(t) if t.contains("sha256") || t.contains("invalid") || t.contains("holds the mod") => {
                ("File check failed".to_string(), 120 * u)
            }
            JoinStage::Failed(_) => ("Download failed".to_string(), 120 * u),
            JoinStage::Rejected(..) => (format!("Can't join {}", j.server), 120 * u),
        };
        let inner = Self::dialog(c, u, 270 * u, height, &title);
        let mut y = inner.y;
        let right = inner.x + inner.w;
        let foot = inner.y + inner.h - 22 * u;
        match &j.stage {
            JoinStage::Prompt | JoinStage::Downloading => {
                if j.stage == JoinStage::Prompt {
                    let line = format!(
                        "{} needs these mods. They are downloaded from the server and used only on {}.",
                        j.server, j.server
                    );
                    for l in wrap(Face::Body, fs, &line, inner.w as f32).into_iter().take(2) {
                        c.text(Face::Body, fs, inner.x as f32, y as f32, TEXT2, &l);
                        y += 8 * u;
                    }
                } else {
                    let (have, total) = j.progress;
                    let t = format!("Getting mods from {}… {} of {}", j.server, size_text(have), size_text(total));
                    c.text(Face::Bold, fs, inner.x as f32, y as f32, TEXT, &t);
                    y += 9 * u;
                    progress(c, u, Rect { x: inner.x, y, w: inner.w, h: 8 * u }, have, total);
                    y += 13 * u;
                }
                if native && j.stage == JoinStage::Prompt {
                    y = self.native_zone(c, u, inner, y, j);
                }
                for (i, m) in j.mods.iter().enumerate() {
                    let r = Rect { x: inner.x, y, w: inner.w, h: row_h };
                    c.bevel(r, u, SLOT, INK);
                    let t = Rect { x: r.x + 3 * u, y: r.y + 3 * u, w: 18 * u, h: 18 * u };
                    c.bevel(t, u, TILES[i % TILES.len()], INK);
                    let initials: String = m.name.chars().filter(|ch| ch.is_alphanumeric()).take(2).collect();
                    c.text(
                        Face::Pixel,
                        7.0 * u as f32,
                        (t.x + 4 * u) as f32,
                        (t.y + 5 * u) as f32,
                        TEXT,
                        &initials.to_uppercase(),
                    );
                    let tx = (t.x + t.w + 4 * u) as f32;
                    let nw =
                        c.text(Face::Bold, fs, tx, (r.y + 3 * u) as f32, TEXT, &format!("{} {}", m.name, m.version));
                    let meta = if j.stage == JoinStage::Downloading {
                        if m.done {
                            " · Checked".to_string()
                        } else if m.have > 0 {
                            format!(" · {} of {}", size_text(m.have), size_text(m.size))
                        } else {
                            " · Waiting".to_string()
                        }
                    } else if m.cached {
                        " · already downloaded".to_string()
                    } else {
                        format!(" · {}", size_text(m.size))
                    };
                    c.text(Face::Body, small, tx + nw, (r.y + 4 * u) as f32, TEXT3, &meta);
                    let mut bx = tx as i32;
                    let by = r.y + 12 * u;
                    if j.stage == JoinStage::Downloading {
                        let (label, kind) = if m.done { ("Done", B_OK) } else { ("Waiting", B_DEFAULT) };
                        Self::badge(c, u, bx, by, label, kind);
                        continue;
                    }
                    if m.native {
                        bx = Self::badge(c, u, bx, by, "NATIVE · FULL ACCESS", B_NATIVE);
                    }
                    if !m.listed {
                        bx = Self::badge(c, u, bx, by, "Not reviewed", B_WARN);
                    }
                    if m.permissions.is_empty() {
                        Self::badge(c, u, bx, by, "Nothing extra", B_DEFAULT);
                    }
                    for p in &m.permissions {
                        let label = match p.as_str() {
                            "channels" => "Talks to the server",
                            "world:read" => "Reads the world",
                            "world:write" => "Changes the world",
                            "players:chat" => "Chat",
                            "net-http" => "Internet",
                            other => other,
                        };
                        bx = Self::badge(c, u, bx, by, label, B_INFO);
                    }
                    y += row_h + 3 * u;
                }
                let note = if j.stage == JoinStage::Downloading {
                    "The game stays responsive. You join as soon as all files are checked.".to_string()
                } else {
                    let total: u64 = j.mods.iter().filter(|m| !m.cached).map(|m| m.size).sum();
                    format!("Total {} to download.", size_text(total))
                };
                c.text(Face::Body, small, inner.x as f32, (foot - 9 * u) as f32, TEXT3, &note);
                c.fill(inner.x - 5 * u + u, foot - 2 * u, inner.w + 10 * u - 2 * u, u, PANEL[2], 1.0);
                if j.stage == JoinStage::Prompt {
                    let can = j.can_accept(self.native_ok);
                    let total: u64 = j.mods.iter().filter(|m| !m.cached).map(|m| m.size).sum();
                    let label = format!("Download and join · {}", size_text(total));
                    let x = self.button(c, u, (right, foot), &label, if can { BTN_PRI } else { BTN_DIS }, Hit::Accept);
                    let cancel = if native { "Don't join" } else { "Cancel" };
                    self.button(c, u, (x - 3 * u, foot), cancel, BTN, Hit::Decline);
                } else {
                    self.button(c, u, (right, foot), "Cancel", BTN, Hit::Cancel);
                }
            }
            JoinStage::Failed(text) | JoinStage::Rejected(text, _) => {
                let url = match &j.stage {
                    JoinStage::Rejected(_, u) => u.clone(),
                    _ => None,
                };
                let lines = wrap(Face::Body, fs, text, (inner.w - 12 * u) as f32);
                let nh = (lines.len() as i32 + url.is_some() as i32) * 8 * u + 8 * u;
                let kind = if matches!(j.stage, JoinStage::Rejected(..)) { N_INFO } else { N_DAN };
                c.bevel(Rect { x: inner.x, y, w: inner.w, h: nh }, u, kind, INK);
                let mut ty = y + 4 * u;
                for l in &lines {
                    c.text(Face::Body, fs, (inner.x + 4 * u) as f32, ty as f32, TEXT, l);
                    ty += 8 * u;
                }
                if let Some(link) = url {
                    c.text(Face::Body, fs, (inner.x + 4 * u) as f32, ty as f32, SUN, &link);
                }
                self.button(c, u, (right, foot), "Back to menu", BTN_PRI, Hit::Dismiss);
            }
        }
    }

    fn native_zone(&mut self, c: &mut Canvas, u: i32, inner: Rect, y: i32, j: &JoinView) -> i32 {
        let h = 40 * u;
        let r = Rect { x: inner.x, y, w: inner.w, h };
        c.bevel(r, u, N_DAN, INK);
        let strip = Rect { x: r.x + u, y: r.y + u, w: r.w - 2 * u, h: 10 * u };
        c.fill(strip.x, strip.y, strip.w, strip.h, 0xC8351F, 1.0);
        for i in (0..strip.w).step_by((8 * u) as usize) {
            c.fill(strip.x + i, strip.y, 4 * u, strip.h, 0x8E2516, 1.0);
        }
        c.shadow_text(
            Face::Pixel,
            7.0 * u as f32,
            ((r.x + 4 * u) as f32, (r.y + 2 * u) as f32),
            (0xFFF6EE, 0x5A140A),
            u as f32,
            "DANGER · NATIVE CODE",
        );
        let names: Vec<&str> = j.mods.iter().filter(|m| m.native).map(|m| m.name.as_str()).collect();
        let refused = j.mods.iter().any(|m| m.refused);
        let msg = if refused {
            format!(
                "{} runs outside the sandbox and is not on the public mod list. It cannot be accepted.",
                names.join(", ")
            )
        } else {
            format!(
                "{} runs outside the sandbox. It can change anything in the game and on this computer.",
                names.join(", ")
            )
        };
        let mut ty = r.y + 13 * u;
        for l in wrap(Face::Body, 5.3 * u as f32, &msg, (r.w - 8 * u) as f32).into_iter().take(2) {
            c.text(Face::Body, 5.3 * u as f32, (r.x + 4 * u) as f32, ty as f32, 0xFFE3DC, &l);
            ty += 7 * u;
        }
        if !refused {
            let b = Rect { x: r.x + 4 * u, y: r.y + h - 11 * u, w: 8 * u, h: 8 * u };
            c.bevel(b, u, FIELD, INK);
            if self.native_ok {
                c.fill(b.x + 2 * u, b.y + 2 * u, 4 * u, 4 * u, SUN, 1.0);
            }
            c.text(
                Face::Body,
                5.3 * u as f32,
                (b.x + 11 * u) as f32,
                (b.y + u) as f32,
                0xFFE3DC,
                &format!("I trust {} and allow full access. (Space)", j.server),
            );
            self.hits.push((Rect { w: r.w - 8 * u, ..b }, Hit::NativeBox));
        }
        y + h + 3 * u
    }

    fn mods_panel(&mut self, c: &mut Canvas, u: i32, v: &View) {
        let w = (150 * u).min(c.w - 8 * u);
        let r = Rect { x: c.w - w - 4 * u, y: 4 * u, w, h: c.h - 8 * u };
        c.fill(0, 0, c.w, c.h, 0x0C0907, 0.45);
        c.bevel(r, u, PANEL, INK);
        let bh = 17 * u;
        c.bevel(Rect { h: bh, ..r }, u, BANNER, INK);
        c.shadow_text(
            Face::Pixel,
            9.0 * u as f32,
            ((r.x + 5 * u) as f32, (r.y + 4 * u) as f32),
            (0xFFF6EA, 0x5A200A),
            u as f32,
            "Mods in this game",
        );
        let close = Rect { x: r.x + r.w - 15 * u, y: r.y + 3 * u, w: 11 * u, h: 11 * u };
        c.bevel(close, u, BTN[..3].try_into().expect("3"), INK);
        c.text(Face::Pixel, 7.0 * u as f32, (close.x + 3 * u) as f32, (close.y + 2 * u) as f32, TEXT, "X");
        self.hits.push((close, Hit::ClosePanel));
        let (fs, small) = (6.0 * u as f32, 5.0 * u as f32);
        let mut y = r.y + bh + 4 * u;
        let x = r.x + 4 * u;
        let iw = r.w - 8 * u;
        if v.mods.is_empty() {
            c.text(Face::Body, fs, x as f32, y as f32, TEXT2, "No mods run in this game.");
            y += 10 * u;
        }
        for (i, (m, origin)) in v.mods.iter().enumerate() {
            let row = Rect { x, y, w: iw, h: 22 * u };
            c.bevel(row, u, SLOT, INK);
            let t = Rect { x: x + 3 * u, y: y + 3 * u, w: 16 * u, h: 16 * u };
            c.bevel(t, u, TILES[i % TILES.len()], INK);
            let initials: String = m.id.chars().filter(|ch| ch.is_alphanumeric()).take(2).collect();
            c.text(
                Face::Pixel,
                6.5 * u as f32,
                (t.x + 3 * u) as f32,
                (t.y + 4 * u) as f32,
                TEXT,
                &initials.to_uppercase(),
            );
            let tx = t.x + t.w + 3 * u;
            c.text(Face::Bold, fs, tx as f32, (y + 2 * u) as f32, TEXT, &format!("{} {}", m.id, m.version));
            let (label, kind) = match m.state {
                "running" => ("Running", B_OK),
                "budget" => ("Turned off · too slow", B_WARN),
                "failed" => ("Failed", B_BAD),
                _ => ("Waiting", B_DEFAULT),
            };
            let bx = Self::badge(c, u, tx, y + 11 * u, label, kind);
            let from = match (*origin, &v.server) {
                ("server", Some(s)) => format!("From {s}"),
                ("dev", _) => "Folder".to_string(),
                _ => "Mine".to_string(),
            };
            let usage = format!("{from} · {:.1} ms/s · peak {:.1} ms", m.usage.ms_last_second, m.usage.peak_ms);
            c.text(Face::Body, small, bx as f32, (y + 12 * u) as f32, TEXT3, &usage);
            y += 25 * u;
        }
        // The log (newest last), in the space left.
        let foot = r.y + r.h - 12 * u;
        let lr = Rect { x, y: y + 2 * u, w: iw, h: (foot - y - 4 * u).max(0) };
        if lr.h > 12 * u {
            c.bevel(lr, u, WELL, INK);
            let line_h = 6 * u;
            let fit = ((lr.h - 4 * u) / line_h).max(0) as usize;
            let start = v.log.len().saturating_sub(fit);
            let mut ly = lr.y + 2 * u;
            for l in &v.log[start..] {
                let rgb = match l.level {
                    "error" => 0xFFB4A6,
                    "warn" => 0xFFDE85,
                    _ => TEXT2,
                };
                let who = l.mod_id.as_deref().map(|m| format!("{m}: ")).unwrap_or_default();
                let mut text = format!("{who}{}", l.text);
                while text.chars().count() > 4 && measure(Face::Body, 4.6 * u as f32, &text) > (lr.w - 6 * u) as f32 {
                    text = text.chars().take(text.chars().count() - 4).collect::<String>() + "…";
                }
                c.text(Face::Body, 4.6 * u as f32, (lr.x + 3 * u) as f32, ly as f32, rgb, &text);
                ly += line_h;
            }
        }
        c.text(
            Face::Body,
            small,
            x as f32,
            (foot + 2 * u) as f32,
            TEXT3,
            if v.dev {
                "F9 or Esc closes · F5 reloads folder mods"
            } else {
                "F9 or Esc closes · turning mods on or off is in the launcher"
            },
        );
    }
}

fn progress(c: &mut Canvas, u: i32, r: Rect, have: u64, total: u64) {
    c.bevel(r, u, WELL, INK);
    let frac = if total == 0 { 1.0 } else { have as f32 / total as f32 };
    let w = ((r.w - 2 * u) as f32 * frac.min(1.0)) as i32;
    c.fill(r.x + u, r.y + u, w, r.h - 2 * u, SUN, 1.0);
    c.fill(r.x + u, r.y + r.h - 2 * u, w, u, 0xC98A2E, 1.0);
}

fn toasts(c: &mut Canvas, u: i32, v: &View) {
    let size = 5.3 * u as f32;
    let mut y = c.h - 24 * u;
    for (_, text) in v.toasts.iter().rev().take(3) {
        let w = (measure(Face::Body, size, text) as i32 + 10 * u).min(120 * u);
        let r = Rect { x: c.w - w - 4 * u, y: y - 14 * u, w, h: 14 * u };
        c.bevel(r, u, PANEL, INK);
        c.fill(r.x + 2 * u, r.y + 3 * u, 2 * u, 8 * u, DANGER, 1.0);
        c.text(Face::Body, size, (r.x + 6 * u) as f32, (r.y + 4 * u) as f32, TEXT, text);
        y -= 16 * u;
    }
}
