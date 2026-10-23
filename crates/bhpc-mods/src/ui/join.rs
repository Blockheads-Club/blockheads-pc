//! The join dialog in the PC-Mods-Join design (notes/design-source `PC-Mods-Join`): a rounded card over a dimmed
//! game with the server's tile, a status tag, the title and address, the mod rows (sides, permissions, size, the
//! switch of an optional mod), the danger zone for native code, the download bar and the failure notices, and
//! footer buttons with their keys. Sizes are the design's pixels at 1280x720 times the window's scale `s`.
//! Fonts are the bundled Liberation Sans (the design's Bricolage Grotesque and Atkinson Hyperlegible are not
//! bundled).

use super::*;
use crate::view::Fail;

/// The design's dark overlay tokens.
const OV_BG: Rgb = 0x100D0B;
const SURF: Rgb = 0x26201C;
const SURF2: Rgb = 0x312924;
const LINE: Rgb = 0x54483F;
const O_TEXT: Rgb = 0xF5EDE5;
const O_TEXT2: Rgb = 0xCBBEB2;
const FOCUS: Rgb = 0xFFD27A;
const SUN_BTN: [Rgb; 3] = [0xFFD27A, 0x1F1A17, 0xC98A2E];
const DANGER_BTN: [Rgb; 3] = [0xFF8F80, 0x1C0D06, 0x8F3A2E];
const DZ_EDGE: Rgb = 0xB42318;
const DZ_BG: Rgb = 0x401D19;
const DZ_STRIPES: [Rgb; 2] = [0xFF8F80, 0xF27466];
const DZ_INK: Rgb = 0x1C0D06;
const PASTELS: [Rgb; 6] = [0xB9D7F5, 0xD9C7F0, 0xBFE8CF, 0xF5DDB9, 0xF0C7D9, 0xC7EDEA];
const INK_ON_PASTEL: Rgb = 0x1F1A17;

/// A chip's look: `(background, text, border)`; `None` border: none.
type Look = (Rgb, Rgb, Option<Rgb>);
const L_CLIENT_SERVER: Look = (0x3B2117, 0xFFB08C, None);
const L_CLIENT: Look = (0x172B45, 0x86B9FF, None);
const L_PERM: Look = (SURF2, O_TEXT2, Some(LINE));
const L_PERM_WARN: Look = (0x3A2F12, 0xF2C14E, Some(0x6E5A22));
const L_EXT: Look = (0x3A2F12, 0xF2C14E, Some(0x8F6F1C));
const L_NATIVE: Look = (0xB42318, 0xFFFFFF, Some(0x7A1710));
const L_OK: Look = (0x163A29, 0x56C990, None);
const L_BAD: Look = (0x401D19, 0xFF8F80, None);
const L_PRIMARY: Look = (0x3B2117, 0xFFB08C, None);
const L_QUIET: Look = (SURF2, O_TEXT2, None);

/// Design pixels to window pixels.
#[derive(Clone, Copy)]
struct Px(f32);

impl Px {
    fn p(self, v: f32) -> i32 {
        (v * self.0).round() as i32
    }

    fn f(self, v: f32) -> f32 {
        v * self.0
    }
}

/// What the keyboard and the pad can step through in the prompt: the native box, then every optional mod.
pub(super) fn focusables(j: &JoinView) -> Vec<Hit> {
    if j.stage != JoinStage::Prompt {
        return Vec::new();
    }
    let mut v = Vec::new();
    if zone(j) && !j.mods.iter().any(|m| m.enabled && m.refused) {
        v.push(Hit::NativeBox);
    }
    v.extend(j.mods.iter().enumerate().filter(|(_, m)| !m.required && !m.refused).map(|(i, _)| Hit::Toggle(i)));
    v
}

/// The danger zone shows when a mod the player takes has native code.
fn zone(j: &JoinView) -> bool {
    j.mods.iter().any(|m| m.native && m.enabled)
}

/// The text "Copy details" puts on the clipboard.
pub(super) fn details(f: &Fail) -> String {
    match &f.kind {
        FailKind::Check { name, version, expected } => {
            format!("{name} {version} does not match its checksum\nexpected sha256 {expected}\n{}", f.text)
        }
        FailKind::Download { name, percent } => format!("{name} stopped at {percent} %\n{}", f.text),
        _ => f.text.clone(),
    }
}

/// `abcd…wxyz` for a long hash.
fn short_hash(h: &str) -> String {
    if h.len() > 12 {
        format!("{}…{}", &h[..4], &h[h.len() - 4..])
    } else {
        h.to_string()
    }
}

/// Two letters for a tile; an address or a name without letters gets `?`.
fn initials(name: &str) -> String {
    let letters: String = name.chars().filter(|c| c.is_alphabetic()).take(2).collect::<String>().to_uppercase();
    if letters.is_empty() {
        "?".to_string()
    } else {
        letters
    }
}

fn mods_word(n: usize) -> String {
    format!("{n} {}", if n == 1 { "mod" } else { "mods" })
}

fn permission_chip(p: &str) -> (String, Look) {
    match p {
        "channels" => ("Channels".into(), L_PERM),
        "world:read" => ("Read world".into(), L_PERM),
        "world:write" => ("Change world".into(), L_PERM_WARN),
        "players:chat" => ("Chat".into(), L_PERM),
        "net-http" => ("Internet".into(), L_PERM_WARN),
        other => (other.to_string(), L_PERM),
    }
}

impl ModUi {
    /// A small label: a rounded chip with the `look`'s colours at `(x, y)`; returns its width.
    fn chip(c: &mut Canvas, px: Px, at: (i32, i32), text: &str, look: Look, pill: bool, draw: bool) -> i32 {
        let size = px.f(12.5);
        let w = measure(Face::Bold, size, text) as i32 + px.p(16.0);
        let h = px.p(22.0);
        if draw {
            let r = Rect { x: at.0, y: at.1, w, h };
            let rad = if pill { h / 2 } else { px.p(7.0) };
            c.rrect(r, rad, look.0, 1.0);
            if let Some(b) = look.2 {
                c.rring(r, rad, px.p(1.5).max(1), b, 1.0);
            }
            c.text(Face::Bold, size, (at.0 + px.p(8.0)) as f32, (at.1 + px.p(3.0)) as f32, look.1, text);
        }
        w
    }

    /// The server's or a mod's tile with its initials.
    fn tile(c: &mut Canvas, px: Px, at: (i32, i32), size: f32, colour: Rgb, label: &str) {
        let n = px.p(size);
        c.rrect(Rect { x: at.0, y: at.1, w: n, h: n }, px.p(size * 0.25), colour, 1.0);
        let fs = px.f(size * 0.36);
        let tw = measure(Face::Bold, fs, label);
        c.text(
            Face::Bold,
            fs,
            at.0 as f32 + (n as f32 - tw) / 2.0,
            at.1 as f32 + (n as f32 - fs * 1.15) / 2.0,
            INK_ON_PASTEL,
            label,
        );
    }

    /// A key hint (`Esc`, `Enter`): the design's `.gl.k`.
    fn key_chip(c: &mut Canvas, px: Px, at: (i32, i32), label: &str, colour: Rgb) -> i32 {
        let size = px.f(12.0);
        let w = (measure(Face::Bold, size, label) as i32 + px.p(16.0)).max(px.p(28.0));
        let r = Rect { x: at.0, y: at.1, w, h: px.p(28.0) };
        c.rrect(r, px.p(6.0), 0x000000, 0.35);
        c.rring(r, px.p(6.0), px.p(2.0), colour, 1.0);
        let tw = measure(Face::Bold, size, label);
        c.text(Face::Bold, size, r.x as f32 + (w as f32 - tw) / 2.0, (r.y + px.p(6.0)) as f32, colour, label);
        w
    }

    /// A footer button whose right edge is `right`; returns its left edge.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        c: &mut Canvas,
        px: Px,
        (right, y): (i32, i32),
        label: &str,
        key: &str,
        look: Option<[Rgb; 3]>,
        enabled: bool,
        focus: bool,
        hit: Hit,
    ) -> i32 {
        let size = px.f(17.0);
        let kw = (measure(Face::Bold, px.f(12.0), key) as i32 + px.p(16.0)).max(px.p(28.0));
        let w = measure(Face::Bold, size, label) as i32 + kw + px.p(10.0) + px.p(44.0);
        let r = Rect { x: right - w, y, w, h: px.p(56.0) };
        let (bg, fg, edge, shade) = match look {
            Some([bg, fg, shade]) => (bg, fg, if bg == SUN_BTN[0] { 0x1F1A17 } else { DZ_EDGE }, shade),
            None => (SURF, O_TEXT, LINE, 0x000000),
        };
        let a = if enabled { 1.0 } else { 0.45 };
        if enabled {
            let drop = Rect { y: r.y + px.p(if look.is_some() { 4.0 } else { 3.0 }), ..r };
            c.rrect(drop, px.p(14.0), shade, 1.0);
        }
        c.rrect(r, px.p(14.0), bg, a);
        c.rring(r, px.p(14.0), px.p(2.0), edge, a);
        if focus {
            let o = px.p(3.0);
            let ring = Rect { x: r.x - o, y: r.y - o, w: r.w + 2 * o, h: r.h + 2 * o };
            c.rring(ring, px.p(14.0) + o, px.p(4.0), FOCUS, 1.0);
        }
        let ky = r.y + (r.h - px.p(28.0)) / 2;
        Self::key_chip(c, px, (r.x + px.p(22.0), ky), key, fg);
        c.text(Face::Bold, size, (r.x + px.p(22.0) + kw + px.p(10.0)) as f32, (r.y + px.p(15.0)) as f32, fg, label);
        self.hits.push((r, hit));
        r.x
    }

    /// The whole join dialog for window scale `s`.
    pub(super) fn join(&mut self, c: &mut Canvas, s: f32, j: &JoinView) {
        let px = Px(s);
        c.fill(0, 0, c.w, c.h, 0x000000, 0.55);
        let w = px.p(680.0).min(c.w - px.p(48.0));
        let pad = px.p(26.0);
        let (head_h, foot_h) = (px.p(112.0), px.p(96.0));
        // Measure the body on a scratch canvas, then draw it into the room the window gives.
        let mut scratch = Canvas::new(1, 1);
        let body_w = w - 2 * pad;
        let body_h = self.body(&mut scratch, px, j, Rect { x: 0, y: 0, w: body_w, h: 0 }, i32::MAX);
        self.hits.clear();
        let h = (head_h + body_h + px.p(24.0) + foot_h).min(c.h - px.p(48.0));
        let r = Rect { x: (c.w - w) / 2, y: (c.h - h) / 2, w, h };
        c.rrect(r, px.p(24.0), OV_BG, 0.94);
        c.rring(r, px.p(24.0), px.p(2.0), LINE, 1.0);
        self.header(c, px, r, j);
        let limit = r.y + h - foot_h;
        self.body(c, px, j, Rect { x: r.x + pad, y: r.y + head_h + px.p(8.0), w: body_w, h: 0 }, limit);
        self.footer(c, px, r, foot_h, j);
    }

    fn header(&mut self, c: &mut Canvas, px: Px, r: Rect, j: &JoinView) {
        let (tag, title): (Option<(String, Look, bool)>, String) = match &j.stage {
            JoinStage::Prompt => {
                let n = j.mods.iter().filter(|m| m.enabled && !m.cached).count();
                let tag = if zone(j) {
                    ("Includes native code".to_string(), L_NATIVE, false)
                } else if n == 0 {
                    ("Nothing to download".to_string(), L_PRIMARY, true)
                } else {
                    (format!("{} to download", mods_word(n)), L_PRIMARY, true)
                };
                (Some(tag), format!("{} uses mods", j.server))
            }
            JoinStage::Downloading => {
                (Some(("Downloading".to_string(), L_PRIMARY, true)), format!("Getting {}'s mods", j.server))
            }
            JoinStage::Failed(f) => {
                let (tag, title) = match &f.kind {
                    FailKind::Download { .. } => ("Download failed", "The download stopped".to_string()),
                    FailKind::Check { .. } => ("File check failed", "A mod failed the file check".to_string()),
                    FailKind::Newer => ("Update needed", "This server needs a newer mod loader".to_string()),
                    FailKind::Other => ("Join failed", format!("Could not join {}", j.server)),
                };
                (Some((tag.to_string(), L_BAD, true)), title)
            }
            JoinStage::Rejected(text, _) if newer(text) => {
                (Some(("Update needed".to_string(), L_BAD, true)), "This server needs a newer mod loader".into())
            }
            JoinStage::Rejected(..) => {
                (Some(("Join refused".to_string(), L_BAD, true)), format!("Can't join {}", j.server))
            }
        };
        let (x, y) = (r.x + px.p(26.0), r.y + px.p(22.0));
        Self::tile(c, px, (x, y), 52.0, PASTELS[2], &initials(&j.server));
        let tx = x + px.p(52.0 + 16.0);
        let mut ty = y;
        if let Some((text, look, pill)) = tag {
            Self::chip(c, px, (tx, ty), &text, look, pill, true);
            ty += px.p(26.0);
        }
        c.text(Face::Bold, px.f(26.0), tx as f32, ty as f32, O_TEXT, &title);
        let addr = if j.address.is_empty() { &j.server } else { &j.address };
        c.text(Face::Body, px.f(13.5), tx as f32, (ty + px.p(32.0)) as f32, O_TEXT2, addr);
    }

    /// Draws (or, on a scratch canvas, measures) the body from `area.y`; returns its height. Rows that would pass
    /// `limit` are left out and counted in a closing line.
    fn body(&mut self, c: &mut Canvas, px: Px, j: &JoinView, area: Rect, limit: i32) -> i32 {
        let gap = px.p(12.0);
        let mut y = area.y;
        match &j.stage {
            JoinStage::Prompt => {
                let optional = j.mods.iter().any(|m| !m.required);
                let text = if optional {
                    "They download once and only run while you play on this server. Optional mods can be switched off."
                } else {
                    "They download once and only run while you play on this server."
                };
                y = Self::paragraph(c, px, area, y, text, 16.0, O_TEXT2) + gap;
                if zone(j) {
                    y = self.danger(c, px, area, y, j) + gap;
                }
                y = self.rows(c, px, area, y, j, limit);
                let total: u64 = j.mods.iter().map(|m| if m.enabled && !m.cached { m.size } else { 0 }).sum();
                c.text(
                    Face::Bold,
                    px.f(17.0),
                    area.x as f32,
                    (y + gap) as f32,
                    O_TEXT,
                    &format!("Total {}", size_text(total)),
                );
                y += gap + px.p(26.0);
            }
            JoinStage::Downloading => {
                let (have, total) = j.progress;
                let pct = (have * 100).checked_div(total).map_or(100, |p| p.min(100));
                let n = j.mods.iter().filter(|m| m.enabled).count();
                c.text(Face::Bold, px.f(17.0), area.x as f32, y as f32, O_TEXT, &mods_word(n));
                let ptxt = format!("{pct} %");
                let pw = measure(Face::Bold, px.f(16.0), &ptxt);
                c.text(Face::Bold, px.f(16.0), area.x as f32 + area.w as f32 - pw, y as f32, O_TEXT, &ptxt);
                y += px.p(26.0) + gap;
                let bar = Rect { x: area.x, y, w: area.w, h: px.p(14.0) };
                c.rrect(bar, px.p(7.0), 0x3A312B, 1.0);
                if pct > 0 {
                    c.rrect(Rect { w: (bar.w * pct as i32 / 100).max(bar.h), ..bar }, px.p(7.0), FOCUS, 1.0);
                }
                y += bar.h + gap;
                let eta = j.eta.map(|s| format!(" · about {s} s left")).unwrap_or_default();
                let line = format!("{} of {}{eta}", size_text(have), size_text(total));
                c.text(Face::Body, px.f(13.5), area.x as f32, y as f32, O_TEXT2, &line);
                y += px.p(22.0) + gap;
                y = self.rows(c, px, area, y, j, limit);
                y = Self::paragraph(
                    c,
                    px,
                    area,
                    y + gap,
                    "After download, every file is checked. Then you join.",
                    14.0,
                    O_TEXT2,
                );
            }
            JoinStage::Failed(f) => {
                y = self.failure(c, px, area, y, j, f, limit);
            }
            JoinStage::Rejected(text, url) => {
                let head = if newer(text) { None } else { Some(text.as_str()) };
                y = Self::callout(
                    c,
                    px,
                    area,
                    y,
                    head,
                    if newer(text) { Some(text.as_str()) } else { None },
                    url.as_deref(),
                );
            }
        }
        y - area.y
    }

    /// Wrapped text at `y`; returns the y below it.
    fn paragraph(c: &mut Canvas, px: Px, area: Rect, y: i32, text: &str, size: f32, colour: Rgb) -> i32 {
        let mut y = y;
        for l in wrap(Face::Body, px.f(size), text, area.w as f32) {
            c.text(Face::Body, px.f(size), area.x as f32, y as f32, colour, &l);
            y += px.p(size * 1.5);
        }
        y
    }

    /// A tinted notice: optional bold headline, text, optional link.
    fn callout(
        c: &mut Canvas,
        px: Px,
        area: Rect,
        y: i32,
        head: Option<&str>,
        text: Option<&str>,
        url: Option<&str>,
    ) -> i32 {
        let inner = area.w - px.p(32.0);
        let mut lines: Vec<(Face, f32, Rgb, String)> = Vec::new();
        if let Some(h) = head {
            lines.extend(
                wrap(Face::Bold, px.f(17.0), h, inner as f32).into_iter().map(|l| (Face::Bold, 17.0, O_TEXT, l)),
            );
        }
        if let Some(t) = text {
            lines.extend(
                wrap(Face::Body, px.f(15.0), t, inner as f32).into_iter().map(|l| (Face::Body, 15.0, O_TEXT2, l)),
            );
        }
        if let Some(u) = url {
            lines.push((Face::Body, 15.0, FOCUS, u.to_string()));
        }
        let h: i32 = lines.iter().map(|l| px.p(l.1 * 1.5)).sum::<i32>() + px.p(28.0);
        let r = Rect { x: area.x, y, w: area.w, h };
        c.rrect(r, px.p(14.0), DZ_BG, 1.0);
        c.rring(r, px.p(14.0), px.p(2.0), DZ_EDGE, 0.7);
        let mut ty = y + px.p(14.0);
        for (face, size, colour, l) in lines {
            c.text(face, px.f(size), (area.x + px.p(16.0)) as f32, ty as f32, colour, &l);
            ty += px.p(size * 1.5);
        }
        y + h
    }

    #[allow(clippy::too_many_arguments)]
    fn failure(&mut self, c: &mut Canvas, px: Px, area: Rect, y: i32, j: &JoinView, f: &Fail, limit: i32) -> i32 {
        let gap = px.p(12.0);
        match &f.kind {
            FailKind::Download { name, percent } => {
                let y =
                    Self::callout(c, px, area, y, Some(&format!("{name} stopped at {percent} %")), Some(&f.text), None);
                self.rows(c, px, area, y + gap, j, limit)
            }
            FailKind::Check { name, version, expected } => {
                let head = format!("{name} {version} doesn't match its checksum");
                let y = Self::callout(c, px, area, y, Some(&head), Some(&f.text), None);
                let code = Rect { x: area.x, y: y + gap, w: area.w, h: px.p(48.0) };
                c.rrect(code, px.p(10.0), 0x0E0B09, 1.0);
                c.rring(code, px.p(10.0), px.p(1.0).max(1), LINE, 1.0);
                let line = format!("expected sha256 {}", short_hash(expected));
                c.text(
                    Face::Body,
                    px.f(13.5),
                    (code.x + px.p(14.0)) as f32,
                    (code.y + px.p(13.0)) as f32,
                    O_TEXT2,
                    &line,
                );
                self.rows(c, px, area, code.y + code.h + gap, j, limit)
            }
            FailKind::Newer => {
                let y = Self::paragraph(c, px, area, y, &f.text, 16.0, O_TEXT2);
                Self::paragraph(
                    c,
                    px,
                    area,
                    y + gap,
                    "Update Blockheads PC from the launcher. Your worlds and mods are kept.",
                    14.0,
                    O_TEXT2,
                )
            }
            FailKind::Other => Self::callout(c, px, area, y, None, Some(&f.text), None),
        }
    }

    /// The danger zone for native code with the confirmation box.
    fn danger(&mut self, c: &mut Canvas, px: Px, area: Rect, y: i32, j: &JoinView) -> i32 {
        let names: Vec<&str> = j.mods.iter().filter(|m| m.native && m.enabled).map(|m| m.name.as_str()).collect();
        let refused = j.mods.iter().any(|m| m.native && m.enabled && m.refused);
        let msg = if refused {
            format!(
                "{} runs outside the sandbox and is not on the public mod list. It cannot be accepted.",
                names.join(", ")
            )
        } else {
            format!(
                "{} runs outside the sandbox. It can change anything in the game and read or change files on this computer.",
                names.join(", ")
            )
        };
        let lines = wrap(Face::Body, px.f(14.5), &msg, (area.w - px.p(28.0)) as f32);
        let head_h = px.p(41.0);
        let body_h =
            px.p(12.0) + lines.len() as i32 * px.p(22.0) + if refused { px.p(12.0) } else { px.p(6.0 + 38.0 + 12.0) };
        let r = Rect { x: area.x, y, w: area.w, h: head_h + body_h };
        c.rrect(r, px.p(14.0), DZ_BG, 1.0);
        // Diagonal stripes behind the headline, clipped to the card's rounded top.
        let stripe = px.p(12.0).max(1);
        for yy in r.y..r.y + head_h {
            for xx in r.x..r.x + r.w {
                let colour = DZ_STRIPES[(((xx - yy).rem_euclid(2 * stripe)) / stripe) as usize];
                c.blend(xx, yy, colour, round_cover(r, px.p(14.0), xx, yy));
            }
        }
        c.rring(r, px.p(14.0), px.p(2.0), DZ_EDGE, 1.0);
        c.text(
            Face::Bold,
            px.f(14.0),
            (r.x + px.p(14.0)) as f32,
            (r.y + px.p(11.0)) as f32,
            DZ_INK,
            "DANGER · NATIVE CODE ON THIS COMPUTER",
        );
        let mut ty = r.y + head_h + px.p(12.0);
        for l in &lines {
            c.text(Face::Body, px.f(14.5), (r.x + px.p(14.0)) as f32, ty as f32, O_TEXT, l);
            ty += px.p(22.0);
        }
        if !refused {
            let row = Rect { x: r.x + px.p(10.0), y: ty + px.p(6.0), w: r.w - px.p(20.0), h: px.p(38.0) };
            let b = Rect { x: row.x + px.p(4.0), y: row.y + px.p(6.0), w: px.p(26.0), h: px.p(26.0) };
            c.rrect(b, px.p(6.0), 0x0E0B09, 1.0);
            c.rring(b, px.p(6.0), px.p(2.0), DZ_STRIPES[0], 1.0);
            if self.native_ok {
                c.rrect(
                    Rect { x: b.x + px.p(6.0), y: b.y + px.p(6.0), w: px.p(14.0), h: px.p(14.0) },
                    px.p(3.0),
                    DZ_STRIPES[0],
                    1.0,
                );
            }
            if focusables(j).get(self.focus) == Some(&Hit::NativeBox) {
                let o = px.p(3.0);
                c.rring(
                    Rect { x: b.x - o, y: b.y - o, w: b.w + 2 * o, h: b.h + 2 * o },
                    px.p(6.0) + o,
                    px.p(3.0),
                    FOCUS,
                    1.0,
                );
            }
            let kx = b.x + b.w + px.p(10.0);
            let kw = Self::key_chip(c, px, (kx, row.y + px.p(5.0)), "Space", O_TEXT2);
            c.text(
                Face::Bold,
                px.f(15.0),
                (kx + kw + px.p(10.0)) as f32,
                (row.y + px.p(9.0)) as f32,
                O_TEXT,
                "I understand. Let it run on this computer.",
            );
            self.hits.push((row, Hit::NativeBox));
        }
        r.y + r.h
    }

    /// The mod rows; returns the y below them. Rows past `limit` become a closing line.
    fn rows(&mut self, c: &mut Canvas, px: Px, area: Rect, y: i32, j: &JoinView, limit: i32) -> i32 {
        let row_h = px.p(74.0);
        let items = focusables(j);
        let focused = items.get(self.focus).copied();
        let optional_any = j.mods.iter().any(|m| !m.required);
        let shown: Vec<(usize, &crate::view::JoinMod)> =
            j.mods.iter().enumerate().filter(|(_, m)| m.enabled || j.stage == JoinStage::Prompt).collect();
        let mut y = y;
        for (n, (i, m)) in shown.iter().enumerate() {
            if y + row_h + px.p(60.0) > limit && n + 1 < shown.len() {
                let more = shown.len() - n;
                c.text(
                    Face::Bold,
                    px.f(15.0),
                    area.x as f32,
                    (y + px.p(4.0)) as f32,
                    O_TEXT2,
                    &format!("and {more} more"),
                );
                y += px.p(30.0);
                break;
            }
            let r = Rect { x: area.x, y, w: area.w, h: row_h };
            let dim = if m.enabled { 1.0 } else { 0.5 };
            c.rrect(r, px.p(14.0), SURF, 1.0);
            if m.refused && m.enabled {
                c.rring(r, px.p(14.0), px.p(2.0), DZ_EDGE, 1.0);
            }
            if focused == Some(Hit::Toggle(*i)) {
                let o = px.p(3.0);
                c.rring(
                    Rect { x: r.x - o, y: r.y - o, w: r.w + 2 * o, h: r.h + 2 * o },
                    px.p(14.0) + o,
                    px.p(3.0),
                    FOCUS,
                    1.0,
                );
            }
            Self::tile(
                c,
                px,
                (r.x + px.p(14.0), r.y + (row_h - px.p(32.0)) / 2),
                32.0,
                PASTELS[i % PASTELS.len()],
                &initials(&m.name),
            );
            let tx = r.x + px.p(14.0 + 32.0 + 14.0);
            // Right side: the size, and the switch (optional mods) or "Required".
            let size = match j.stage {
                JoinStage::Prompt if !m.enabled => "Off".to_string(),
                JoinStage::Prompt if m.cached => "Downloaded".to_string(),
                JoinStage::Prompt => size_text(m.size),
                _ => String::new(),
            };
            let mut rx = r.x + r.w - px.p(14.0);
            let colour = if m.enabled { O_TEXT2 } else { 0x9C8E82 };
            if !size.is_empty() {
                let sw = measure(Face::Body, px.f(15.0), &size) as i32;
                c.text(
                    Face::Body,
                    px.f(15.0),
                    (rx - sw) as f32,
                    (r.y + (row_h - px.p(22.0)) / 2) as f32,
                    colour,
                    &size,
                );
                rx -= sw + px.p(14.0);
            }
            match j.stage {
                JoinStage::Prompt if optional_any && m.required => {
                    let w = Self::chip(c, px, (0, 0), "Required", L_QUIET, true, false);
                    Self::chip(c, px, (rx - w, r.y + (row_h - px.p(22.0)) / 2), "Required", L_QUIET, true, true);
                    rx -= w + px.p(14.0);
                }
                JoinStage::Prompt if !m.required => {
                    let t =
                        Rect { x: rx - px.p(46.0), y: r.y + (row_h - px.p(28.0)) / 2, w: px.p(46.0), h: px.p(28.0) };
                    Self::switch(c, px, t, m.enabled, !m.refused);
                    rx = t.x - px.p(14.0);
                    if !m.refused {
                        self.hits.push((r, Hit::Toggle(*i)));
                    }
                }
                JoinStage::Downloading => {
                    let (label, look) = if m.done {
                        ("Done".to_string(), Some(L_OK))
                    } else if m.have > 0 {
                        (format!("{} %", (m.have * 100 / m.size.max(1)).min(100)), None)
                    } else {
                        ("Waiting".to_string(), Some(L_QUIET))
                    };
                    let w = Self::chip(c, px, (0, 0), &label, look.unwrap_or(L_QUIET), true, false);
                    match look {
                        Some(l) => {
                            Self::chip(c, px, (rx - w, r.y + (row_h - px.p(22.0)) / 2), &label, l, true, true);
                        }
                        None => {
                            c.text(
                                Face::Bold,
                                px.f(15.0),
                                (rx - w + px.p(8.0)) as f32,
                                (r.y + (row_h - px.p(22.0)) / 2) as f32,
                                O_TEXT,
                                &label,
                            );
                        }
                    }
                    rx -= w + px.p(14.0);
                }
                JoinStage::Failed(_) => {
                    if m.done {
                        let w = Self::chip(c, px, (0, 0), "Done", L_OK, true, false);
                        Self::chip(c, px, (rx - w, r.y + (row_h - px.p(22.0)) / 2), "Done", L_OK, true, true);
                        rx -= w + px.p(14.0);
                    } else if m.have > 0 {
                        let w = Self::chip(c, px, (0, 0), "Failed", L_BAD, true, false);
                        Self::chip(c, px, (rx - w, r.y + (row_h - px.p(22.0)) / 2), "Failed", L_BAD, true, true);
                        rx -= w + px.p(14.0);
                    }
                }
                _ => {}
            }
            // Name and version, then the chips (as many as fit).
            let nw = c.text(
                Face::Bold,
                px.f(17.0),
                tx as f32,
                (r.y + px.p(12.0)) as f32,
                if m.enabled { O_TEXT } else { O_TEXT2 },
                &m.name,
            );
            c.text(Face::Body, px.f(14.0), tx as f32 + nw + px.f(8.0), (r.y + px.p(14.0)) as f32, colour, &m.version);
            let mut chips: Vec<(String, Look)> = vec![if m.with_server {
                ("Client + server".into(), L_CLIENT_SERVER)
            } else {
                ("Client only".into(), L_CLIENT)
            }];
            if j.stage == JoinStage::Prompt && m.enabled {
                if m.native {
                    chips.push(("Native · full access".into(), L_NATIVE));
                }
                if m.refused {
                    chips.push(("Not on the public list".into(), L_BAD));
                }
                if !m.listed && !m.native {
                    chips.push(("Not reviewed".into(), L_EXT));
                }
                chips.extend(m.permissions.iter().map(|p| permission_chip(p)));
            }
            let cy = r.y + px.p(42.0);
            let mut cx = tx;
            for (text, look) in chips {
                let w = Self::chip(c, px, (0, 0), &text, look, false, false);
                if cx + w > rx {
                    break;
                }
                Self::chip(c, px, (cx, cy), &text, look, false, true);
                cx += w + px.p(4.0);
            }
            if dim < 1.0 {
                c.rrect(r, px.p(14.0), OV_BG, 0.35);
            }
            y += row_h + px.p(8.0);
        }
        y - px.p(8.0)
    }

    /// The design's `.sw`: a pill track with a knob.
    fn switch(c: &mut Canvas, px: Px, t: Rect, on: bool, enabled: bool) {
        let a = if enabled { 1.0 } else { 0.45 };
        let (fill, edge, knob) = if on { (0xFF7A45, 0x9C3A12, 0x1C0D06) } else { (SURF2, LINE, 0xAE9F92) };
        c.rrect(t, t.h / 2, fill, a);
        c.rring(t, t.h / 2, px.p(2.0), edge, a);
        let k = px.p(18.0);
        let kx = if on { t.x + t.w - k - px.p(5.0) } else { t.x + px.p(5.0) };
        c.rrect(Rect { x: kx, y: t.y + (t.h - k) / 2, w: k, h: k }, k / 2, knob, a);
    }

    fn footer(&mut self, c: &mut Canvas, px: Px, r: Rect, foot_h: i32, j: &JoinView) {
        let top = r.y + r.h - foot_h;
        c.fill(r.x + px.p(2.0), top, r.w - px.p(4.0), px.p(2.0), LINE, 1.0);
        let y = top + px.p(18.0);
        let right = r.x + r.w - px.p(26.0);
        let gap = px.p(12.0);
        match &j.stage {
            JoinStage::Prompt => {
                let native = zone(j);
                let can = j.can_accept(self.native_ok);
                if native {
                    let x = self.button(
                        c,
                        px,
                        (right, y),
                        "Download and join",
                        "Enter",
                        Some(DANGER_BTN),
                        can,
                        false,
                        Hit::Accept,
                    );
                    self.button(c, px, (x - gap, y), "Don't join", "Esc", Some(SUN_BTN), true, false, Hit::Decline);
                } else {
                    let x = self.button(
                        c,
                        px,
                        (right, y),
                        "Download and join",
                        "Enter",
                        Some(SUN_BTN),
                        can,
                        false,
                        Hit::Accept,
                    );
                    self.button(c, px, (x - gap, y), "Cancel", "Esc", None, true, false, Hit::Decline);
                }
            }
            JoinStage::Downloading => {
                self.button(c, px, (right, y), "Cancel", "Esc", None, true, true, Hit::Cancel);
            }
            JoinStage::Failed(f) => {
                let x = self.button(
                    c,
                    px,
                    (right, y),
                    "Back to server list",
                    "Enter",
                    Some(SUN_BTN),
                    true,
                    false,
                    Hit::Dismiss,
                );
                if matches!(f.kind, FailKind::Check { .. }) {
                    self.button(c, px, (x - gap, y), "Copy details", "Y", None, true, false, Hit::Copy);
                }
            }
            JoinStage::Rejected(..) => {
                self.button(
                    c,
                    px,
                    (right, y),
                    "Back to server list",
                    "Enter",
                    Some(SUN_BTN),
                    true,
                    false,
                    Hit::Dismiss,
                );
            }
        }
    }
}

/// A server text that asks for a newer mod loader (the 0xBC protocol mismatch).
fn newer(text: &str) -> bool {
    text.to_lowercase().contains("mod loader")
}
