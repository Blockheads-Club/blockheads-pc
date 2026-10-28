//! The mods' HUD in the IG kit: widgets in the top corners, notices at the top centre, and the dialog.

use super::*;
use crate::hud::{ModDialog, ModWidget, Notice};
use bhmod_core::ui::{ButtonKind, NoticeLevel, Slot, WidgetKind, CLOSED};

/// `[light, mid, dark]` of a notice level's stripe.
fn level_colour(l: NoticeLevel) -> Rgb {
    match l {
        NoticeLevel::Info => TILES[2][1],
        NoticeLevel::Warning => B_WARN[2],
        NoticeLevel::Success => B_OK[2],
        NoticeLevel::Error => DANGER,
    }
}

fn button_colours(k: ButtonKind) -> [Rgb; 3] {
    match k {
        ButtonKind::Primary => BANNER,
        ButtonKind::Secondary => [BTN[0], BTN[1], BTN[2]],
        ButtonKind::Danger => [0xE0644F, DANGER, 0x8E2316],
    }
}

impl ModUi {
    /// The widgets of the mods, stacked in their corners.
    pub(super) fn widgets(&mut self, c: &mut Canvas, u: i32, widgets: &[ModWidget]) {
        let (mut left, mut right) = (4 * u, 20 * u);
        let small = 4.8 * u as f32;
        let fs = 6.0 * u as f32;
        for w in widgets {
            let w = &w.widget;
            let max = 90 * u;
            let title = w.title.as_deref().map(|t| cut(u, t, max - 6 * u));
            let lines: Vec<String> = match w.kind {
                WidgetKind::Panel => {
                    wrap(Face::Body, small, &w.text, (max - 6 * u) as f32).into_iter().take(4).collect()
                }
                _ => vec![cut(u, &w.text, max - 6 * u)],
            };
            let (face, size, lh) = match w.kind {
                WidgetKind::Counter => (Face::Pixel, 9.0 * u as f32, 11 * u),
                _ => (Face::Body, small, 6 * u),
            };
            let text_w = lines.iter().map(|l| measure(face, size, l) as i32).max().unwrap_or(0);
            let title_w = title.as_deref().map_or(0, |t| measure(Face::Bold, fs, t) as i32);
            let bw = (text_w.max(title_w) + 8 * u).clamp(30 * u, max);
            let bh = 4 * u + title.as_ref().map_or(0, |_| 8 * u) + lines.len() as i32 * lh;
            let (x, y) = match w.slot {
                Slot::TopLeft => (4 * u, left),
                Slot::TopRight => (c.w - bw - 4 * u, right),
            };
            if y + bh > c.h {
                continue;
            }
            c.bevel(Rect { x, y, w: bw, h: bh }, u, PANEL, INK);
            let mut ty = y + 2 * u;
            if let Some(t) = &title {
                c.text(Face::Bold, fs, (x + 4 * u) as f32, ty as f32, TEXT, t);
                ty += 8 * u;
            }
            for l in &lines {
                c.text(face, size, (x + 4 * u) as f32, ty as f32, TEXT2, l);
                ty += lh;
            }
            match w.slot {
                Slot::TopLeft => left += bh + 2 * u,
                Slot::TopRight => right += bh + 2 * u,
            }
        }
    }

    /// Notices at the top centre, newest last.
    pub(super) fn notices(&mut self, c: &mut Canvas, u: i32, notices: &[Notice]) {
        let size = 5.5 * u as f32;
        let mut y = 4 * u;
        for n in notices {
            let w = (180 * u).min(c.w - 8 * u);
            let lines = wrap(Face::Body, size, &n.text, (w - 12 * u) as f32);
            let h = 6 * u + lines.len() as i32 * 8 * u;
            let r = Rect { x: (c.w - w) / 2, y, w, h };
            c.bevel(r, u, PANEL, INK);
            c.fill(r.x + 2 * u, r.y + 2 * u, 2 * u, h - 4 * u, level_colour(n.level), 1.0);
            let mut ty = r.y + 3 * u;
            for l in &lines {
                c.text(Face::Body, size, (r.x + 7 * u) as f32, ty as f32, TEXT, l);
                ty += 8 * u;
            }
            y += h + 2 * u;
        }
    }

    /// The text field's text of dialog `d` (its initial text when it just opened).
    fn field_text(&mut self, d: &ModDialog) -> String {
        if self.dialog_for != Some(d.id) {
            self.dialog_for = Some(d.id);
            self.dialog_focus = 0;
            self.dialog_text = d.dialog.field.as_ref().map(|f| f.initial.clone()).unwrap_or_default();
        }
        self.dialog_text.clone()
    }

    pub(super) fn answer(&mut self, d: &ModDialog, button: u32) -> Option<UiCmd> {
        let text = d.dialog.field.as_ref().map(|_| self.dialog_text.clone());
        self.dialog_for = None;
        Some(UiCmd::DialogAnswer { id: d.id, button, text })
    }

    pub(super) fn dialog_input(&mut self, ev: UiInput, v: &View, d: &ModDialog) -> Option<UiCmd> {
        self.field_text(d);
        let n = d.dialog.buttons.len().max(1);
        let field = d.dialog.field.as_ref();
        match ev {
            UiInput::Key(HID_ENTER) => self.answer(d, if field.is_some() { 0 } else { self.dialog_focus as u32 }),
            UiInput::Key(HID_ESCAPE) => self.answer(d, CLOSED),
            UiInput::Key(HID_LEFT) => {
                self.dialog_focus = (self.dialog_focus + n - 1) % n;
                None
            }
            UiInput::Key(HID_RIGHT | HID_TAB) => {
                self.dialog_focus = (self.dialog_focus + 1) % n;
                None
            }
            UiInput::Key(HID_BACKSPACE) => {
                self.dialog_text.pop();
                None
            }
            UiInput::Char(ch) => {
                if field.is_some_and(|f| self.dialog_text.chars().count() < f.max_length as usize) && !ch.is_control() {
                    self.dialog_text.push(ch);
                }
                None
            }
            UiInput::Click(x, y) => self.clicked(x, y).and_then(|h| self.act(h, v)),
            _ => None,
        }
    }

    pub(super) fn dialog(&mut self, c: &mut Canvas, u: i32, d: &ModDialog) {
        let text = self.field_text(d);
        let w = (170 * u).min(c.w - 8 * u);
        let size = 5.5 * u as f32;
        let lines = wrap(Face::Body, size, &d.dialog.text, (w - 10 * u) as f32);
        let field_h = if d.dialog.field.is_some() { 16 * u } else { 0 };
        let h = 17 * u + 6 * u + lines.len() as i32 * 8 * u + field_h + 18 * u;
        let r = Rect { x: (c.w - w) / 2, y: ((c.h - h) / 2).max(0), w, h };
        c.fill(0, 0, c.w, c.h, 0x0C0907, 0.45);
        c.bevel(r, u, PANEL, INK);
        c.bevel(Rect { h: 17 * u, ..r }, u, BANNER, INK);
        let title = cut(u, &d.dialog.title, r.w - 10 * u);
        c.shadow_text(
            Face::Pixel,
            9.0 * u as f32,
            ((r.x + 5 * u) as f32, (r.y + 4 * u) as f32),
            (0xFFF6EA, 0x5A200A),
            u as f32,
            &title,
        );
        let mut y = r.y + 23 * u;
        for l in &lines {
            c.text(Face::Body, size, (r.x + 5 * u) as f32, y as f32, TEXT, l);
            y += 8 * u;
        }
        if let Some(f) = &d.dialog.field {
            let fr = Rect { x: r.x + 5 * u, y, w: r.w - 10 * u, h: 12 * u };
            c.bevel(fr, u, WELL, INK);
            let (shown, rgb) =
                if text.is_empty() { (f.placeholder.clone(), TEXT3) } else { (format!("{text}|"), TEXT) };
            c.text(Face::Body, size, (fr.x + 3 * u) as f32, (fr.y + 3 * u) as f32, rgb, &cut(u, &shown, fr.w - 6 * u));
        }
        let mut bx = r.x + r.w - 5 * u;
        for (i, b) in d.dialog.buttons.iter().enumerate().rev() {
            let bw = (measure(Face::Bold, size, &b.label) as i32 + 10 * u).max(30 * u);
            let br = Rect { x: bx - bw, y: r.y + r.h - 15 * u, w: bw, h: 11 * u };
            c.bevel(br, u, button_colours(b.kind), INK);
            if i == self.dialog_focus {
                c.rring(Rect { x: br.x - u, y: br.y - u, w: br.w + 2 * u, h: br.h + 2 * u }, 2 * u, u, TEXT, 1.0);
            }
            let tw = measure(Face::Bold, size, &b.label) as i32;
            c.text(Face::Bold, size, (br.x + (br.w - tw) / 2) as f32, (br.y + 2 * u) as f32, TEXT, &b.label);
            self.hits.push((br, Hit::DialogButton(i)));
            bx = br.x - 3 * u;
        }
    }
}
