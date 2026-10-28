//! The settings screen of a mod ([`crate::form`]) and the microphone opt-in dialog, in the IG kit.

use super::*;
use crate::form::{RowValue, SettingsView};

/// The command for one step of row `i` (`None` when it cannot change that way).
pub(super) fn change(v: &View, i: usize, dir: i32) -> Option<UiCmd> {
    let s = v.settings.as_ref()?;
    let row = s.rows.get(i)?;
    let value = row.step(dir)?;
    Some(UiCmd::Setting { section: row.section, mod_id: s.mod_id.clone(), key: row.key.clone(), value })
}

fn button(c: &mut Canvas, u: i32, r: Rect, label: &str, colours: [Rgb; 3]) {
    c.bevel(r, u, colours, INK);
    let size = 5.5 * u as f32;
    let tw = measure(Face::Bold, size, label) as i32;
    c.text(Face::Bold, size, (r.x + (r.w - tw) / 2) as f32, (r.y + (r.h - 7 * u) / 2) as f32, TEXT, label);
}

const BTN3: [Rgb; 3] = [BTN[0], BTN[1], BTN[2]];
const ON: [Rgb; 3] = [0x82B694, 0x3E8E5A, 0x2B633F];

impl ModUi {
    pub(super) fn settings_input(&mut self, ev: UiInput, v: &View, s: &SettingsView) -> Option<UiCmd> {
        let n = s.rows.len();
        self.sel = self.sel.min(n.saturating_sub(1));
        if self.capture {
            self.capture = false;
            let UiInput::Key(k) = ev else { return None };
            let row = s.rows.get(self.sel)?;
            let rebind = |key: Option<&str>| UiCmd::Rebind {
                mod_id: s.mod_id.clone(),
                action: row.key.clone(),
                key: key.map(str::to_string),
            };
            return match k {
                HID_ESCAPE => None,
                HID_BACKSPACE | HID_DELETE => Some(rebind(None)),
                k => bhpc_input::keymap::api_key_name(k).map(|name| rebind(Some(name))),
            };
        }
        let selected_key = matches!(s.rows.get(self.sel).map(|r| &r.value), Some(RowValue::Key(_)));
        match ev {
            UiInput::Key(HID_ESCAPE) => self.act(Hit::CloseSettings, v),
            UiInput::Key(k @ (HID_UP | HID_DOWN)) if n > 0 => {
                self.sel = if k == HID_DOWN { (self.sel + 1) % n } else { (self.sel + n - 1) % n };
                None
            }
            UiInput::Key(HID_LEFT) => change(v, self.sel, -1),
            UiInput::Key(HID_RIGHT) => change(v, self.sel, 1),
            UiInput::Key(HID_ENTER | HID_SPACE) if selected_key => self.act(Hit::SettingKey(self.sel), v),
            UiInput::Key(HID_ENTER | HID_SPACE) => change(v, self.sel, 1),
            UiInput::Click(x, y) => self.clicked(x, y).and_then(|h| self.act(h, v)),
            _ => None,
        }
    }

    pub(super) fn settings_screen(&mut self, c: &mut Canvas, u: i32, s: &SettingsView) {
        let w = (200 * u).min(c.w - 8 * u);
        let r = Rect { x: (c.w - w) / 2, y: 4 * u, w, h: c.h - 8 * u };
        c.fill(0, 0, c.w, c.h, 0x0C0907, 0.55);
        c.bevel(r, u, PANEL, INK);
        let bh = 17 * u;
        c.bevel(Rect { h: bh, ..r }, u, BANNER, INK);
        let title = cut(u, &format!("Settings · {}", s.title), r.w - 30 * u);
        c.shadow_text(
            Face::Pixel,
            9.0 * u as f32,
            ((r.x + 5 * u) as f32, (r.y + 4 * u) as f32),
            (0xFFF6EA, 0x5A200A),
            u as f32,
            &title,
        );
        let close = Rect { x: r.x + r.w - 15 * u, y: r.y + 3 * u, w: 11 * u, h: 11 * u };
        button(c, u, close, "X", BTN3);
        self.hits.push((close, Hit::CloseSettings));
        let (fs, small) = (6.0 * u as f32, 4.8 * u as f32);
        let x = r.x + 4 * u;
        let iw = r.w - 8 * u;
        let foot = r.y + r.h - 12 * u;
        let top = r.y + bh + 3 * u;
        if s.rows.is_empty() {
            c.text(Face::Body, fs, x as f32, top as f32, TEXT2, "This mod has no settings here.");
        }
        let (rh, hh) = (19 * u, 10 * u);
        // The first row drawn: the selected row must fit above the footer.
        let height = |from: usize, to: usize| -> i32 {
            (from..=to).map(|i| rh + if i == from || s.rows[i].section != s.rows[i - 1].section { hh } else { 0 }).sum()
        };
        let sel = self.sel.min(s.rows.len().saturating_sub(1));
        let mut start = 0;
        while start < sel && top + height(start, sel) > foot {
            start += 1;
        }
        let mut y = top;
        for (i, row) in s.rows.iter().enumerate().skip(start) {
            let header = i == start || row.section != s.rows[i - 1].section;
            if y + rh + if header { hh } else { 0 } > foot {
                break;
            }
            if header {
                c.text(Face::Pixel, 5.5 * u as f32, x as f32, (y + 2 * u) as f32, TEXT3, row.section.title());
                y += hh;
            }
            let rr = Rect { x, y, w: iw, h: rh - 2 * u };
            c.bevel(rr, u, if i == sel { WELL } else { SLOT }, INK);
            let cw = 64 * u;
            let label_w = iw - cw - 8 * u;
            let label = if row.restart { format!("{} (restart)", row.label) } else { row.label.clone() };
            c.text(Face::Bold, fs, (x + 3 * u) as f32, (y + 2 * u) as f32, TEXT, &cut(u, &label, label_w));
            if let Some(h) = &row.help {
                c.text(Face::Body, small, (x + 3 * u) as f32, (y + 9 * u) as f32, TEXT3, &cut(u, h, label_w));
            }
            let ctl = Rect { x: rr.x + rr.w - cw - 3 * u, y: y + 3 * u, w: cw, h: 11 * u };
            match &row.value {
                RowValue::Bool(on) => {
                    button(c, u, ctl, &row.value.text(), if *on { ON } else { BTN3 });
                    self.hits.push((ctl, Hit::SettingInc(i)));
                }
                RowValue::Number { .. } | RowValue::Choice { .. } => {
                    let a = Rect { w: 11 * u, ..ctl };
                    let b = Rect { x: ctl.x + ctl.w - 11 * u, ..a };
                    button(c, u, a, "<", BTN3);
                    button(c, u, b, ">", BTN3);
                    let t = cut(u, &row.value.text(), ctl.w - 24 * u);
                    let tw = measure(Face::Bold, fs, &t) as i32;
                    c.text(Face::Bold, fs, (ctl.x + (ctl.w - tw) / 2) as f32, (ctl.y + 2 * u) as f32, TEXT, &t);
                    self.hits.push((a, Hit::SettingDec(i)));
                    self.hits.push((b, Hit::SettingInc(i)));
                }
                RowValue::Key(_) => {
                    let t = if self.capture && i == sel { "Press a key".to_string() } else { row.value.text() };
                    button(c, u, ctl, &t, if self.capture && i == sel { ON } else { BTN3 });
                    self.hits.push((ctl, Hit::SettingKey(i)));
                }
                RowValue::Fixed(t) => {
                    c.text(Face::Body, small, ctl.x as f32, (ctl.y + 3 * u) as f32, TEXT2, &cut(u, t, ctl.w));
                }
            }
            y += rh;
        }
        let (rgb, text) = match &s.status {
            Some((true, t)) => (0x8FE5B4, t.clone()),
            Some((false, t)) => (0xFFB4A6, t.clone()),
            None => (TEXT3, "↑↓ select · ←→ change · Enter: key or switch · Esc closes".to_string()),
        };
        c.text(Face::Body, small, x as f32, (foot + 2 * u) as f32, rgb, &cut(u, &text, iw));
    }

    pub(super) fn mic_dialog(&mut self, c: &mut Canvas, u: i32) {
        let w = (150 * u).min(c.w - 8 * u);
        let size = 5.5 * u as f32;
        let text = "A mod of this server wants your microphone for voice chat. It records only while you talk, \
                    and the red microphone shows when it does. You can change this later in the settings.";
        let lines = wrap(Face::Body, size, text, (w - 10 * u) as f32);
        let h = 17 * u + 6 * u + lines.len() as i32 * 8 * u + 18 * u;
        let r = Rect { x: (c.w - w) / 2, y: (c.h - h) / 2, w, h };
        c.fill(0, 0, c.w, c.h, 0x0C0907, 0.55);
        c.bevel(r, u, PANEL, INK);
        c.bevel(Rect { h: 17 * u, ..r }, u, BANNER, INK);
        c.shadow_text(
            Face::Pixel,
            9.0 * u as f32,
            ((r.x + 5 * u) as f32, (r.y + 4 * u) as f32),
            (0xFFF6EA, 0x5A200A),
            u as f32,
            "Use the microphone?",
        );
        let mut y = r.y + 23 * u;
        for l in &lines {
            c.text(Face::Body, size, (r.x + 5 * u) as f32, y as f32, TEXT, l);
            y += 8 * u;
        }
        let bw = 50 * u;
        let allow = Rect { x: r.x + r.w - bw - 5 * u, y: r.y + r.h - 15 * u, w: bw, h: 11 * u };
        let deny = Rect { x: allow.x - bw - 3 * u, ..allow };
        button(c, u, allow, "Allow (Enter)", ON);
        button(c, u, deny, "Not now (Esc)", BTN3);
        self.hits.push((allow, Hit::MicAllow));
        self.hits.push((deny, Hit::MicDeny));
    }
}
