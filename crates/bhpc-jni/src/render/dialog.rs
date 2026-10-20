//! The dialog of an alert: the system alerts with buttons only ("Exit?", the privacy consent, "Restore
//! Double-Time") and the ones with an input line (the "World Name" prompt and the other name prompts).
//!
//! On Android `AlertView` becomes a system `AlertDialog` (with an `EditText` for an input line): the system draws it
//! and the soft keyboard types into it. The game's own views hold nothing to draw (the alert has no frame), so the
//! host draws the dialog over everything: title, message, the field with the text and the caret, and the buttons.
//! The dialog is modal: while it is shown it takes every touch, and a click on one of its buttons is kept for the
//! runner ([`Vm::take_dialog_pick`]), which answers the alert with it. Keys and controller buttons move the focus
//! between the buttons and press them ([`Vm::dialog_nav`]). Alerts shown while one is up wait their turn.

use super::{measure, wrap, Canvas, FontRef, Item, Rect};
use crate::ui::{TextStyle, TouchPhase};
use crate::vm::{Alert, Vm};
use ab_glyph::{FontArc, ScaleFont};
use std::sync::OnceLock;

/// The shown dialog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dialog {
    pub alert: Alert,
    /// Where the caret is in the field's text (UTF-16 units); `None` draws none.
    pub caret: Option<usize>,
    /// The button a touch went down on.
    pub pressed: Option<i32>,
    /// The button the keyboard or controller would press: the default (positive) one at first.
    pub focus: Option<i32>,
    /// The button the user clicked, until the runner takes it.
    pub pick: Option<i32>,
}

/// Where the parts of the dialog lie, in window pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    /// Size factor for large windows (1 up to a 768 pixel high window).
    pub scale: f32,
    pub panel: Rect,
    pub title: Rect,
    pub message: Option<Rect>,
    /// The input line, for an alert with a text field.
    pub field: Option<Rect>,
    /// Caption, button index, rectangle: the cancel button first (left), the way Android lays out a dialog; more
    /// than three are stacked.
    pub buttons: Vec<(String, i32, Rect)>,
}

const SCRIM: u32 = 0x9900_0000;
const PANEL: u32 = 0xFF2B_2B2B;
const WHITE: u32 = 0xFFFF_FFFF;
const GREY: u32 = 0xFFD0_D0D0;
const BLACK: u32 = 0xFF00_0000;
const BUTTON: u32 = 0xFF3D_3D3D;
/// The focused button (keyboard, controller).
const FOCUS: u32 = 0xFF5A_5A5A;
/// Android's (Holo) accent: the field's underline and a pressed button.
const ACCENT: u32 = 0xFF33_B5E5;

/// The bundled regular and bold faces (keys as the view renderer names them, so the items compare alike).
fn fonts() -> &'static [FontRef; 2] {
    static CELL: OnceLock<[FontRef; 2]> = OnceLock::new();
    CELL.get_or_init(|| {
        [(0, bhpc_vfs::fonts::Style::Regular), (crate::jlib::BOLD, bhpc_vfs::fonts::Style::Bold)].map(|(bits, s)| {
            FontRef {
                key: super::bundled_key(bits),
                font: FontArc::try_from_vec(bhpc_vfs::fonts::bundled(s).to_vec()).expect("the bundled fonts parse"),
            }
        })
    })
}

fn style(size: f32, color: u32, align: i32, lines: i32) -> TextStyle {
    TextStyle { size, color, align, lines, min_size: if lines == 1 { size / 2.0 } else { 0.0 }, ..TextStyle::default() }
}

fn message_style(s: f32) -> TextStyle {
    style(15.0 * s, GREY, 0, 0)
}

fn field_style(s: f32) -> TextStyle {
    TextStyle { min_size: 0.0, ..style(17.0 * s, BLACK, 0, 1) }
}

/// The parts of the dialog of `alert` on a window of `screen` pixels, centred on it.
pub(crate) fn layout(screen: (f32, f32), alert: &Alert, font: &FontArc) -> Layout {
    let (sw, sh) = screen;
    let s = (sw.min(sh) / 768.0).max(1.0);
    let width = (440.0 * s).min(sw - 32.0 * s).max(160.0 * s);
    let pad = 20.0 * s;
    let inner = width - 2.0 * pad;
    let title_h = 28.0 * s;
    let message_h = if alert.message.is_empty() {
        0.0
    } else {
        let st = message_style(s);
        let sf = Canvas::scaled_for([0.0, 0.0, inner, 0.0], &alert.message, font, &st);
        wrap(&sf, &alert.message, inner, 0).len() as f32 * (sf.height() + sf.line_gap())
    };
    let (gap, button_h) = (12.0 * s, 40.0 * s);
    let field_h = if alert.field.is_some() { 36.0 * s + 20.0 * s } else { 8.0 * s };
    let all: Vec<(String, i32)> = alert.cancel.iter().chain(&alert.buttons).cloned().collect();
    let bgap = 8.0 * s;
    let stacked = all.len() > 3;
    let buttons_h = if stacked { all.len() as f32 * (button_h + bgap) - bgap } else { button_h };
    let height = pad + title_h + gap + message_h + if message_h > 0.0 { gap } else { 0.0 } + field_h + buttons_h + pad;
    let x0 = ((sw - width) / 2.0).round();
    let y0 = ((sh - height) / 2.0).max(0.0).round();
    let (l, r) = (x0 + pad, x0 + width - pad);
    let mut y = y0 + pad;
    let title = [l, y, r, y + title_h];
    y += title_h + gap;
    let message = (message_h > 0.0).then(|| {
        let m = [l, y, r, y + message_h];
        y += message_h + gap;
        m
    });
    let field = alert.field.map(|_| [l, y, r, y + 36.0 * s]);
    y += field_h;
    let bw = if stacked { inner } else { (inner - bgap * (all.len().max(1) - 1) as f32) / all.len().max(1) as f32 };
    let buttons = all
        .into_iter()
        .enumerate()
        .map(|(i, (caption, index))| {
            let (bx, by) =
                if stacked { (l, y + i as f32 * (button_h + bgap)) } else { (l + i as f32 * (bw + bgap), y) };
            (caption, index, [bx, by, bx + bw, by + button_h])
        })
        .collect();
    Layout { scale: s, panel: [x0, y0, x0 + width, y0 + height], title, message, field, buttons }
}

fn inside(r: &Rect, x: f32, y: f32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// What the dialog draws, back to front, over a window of `screen` pixels; `text` is the field's.
pub(super) fn items(d: &Dialog, l: &Layout, screen: (f32, f32), text: &str) -> Vec<Item> {
    let [regular, bold] = fonts();
    let s = l.scale;
    let all = [0.0, 0.0, screen.0, screen.1];
    let fill = |rect: Rect, clip: Rect, argb: u32| Item::Fill { rect, clip, argb, alpha: 1.0 };
    let label = |rect: Rect, clip: Rect, text: &str, font: &FontRef, style: TextStyle| Item::Text {
        rect,
        clip,
        alpha: 1.0,
        text: text.to_string(),
        font: font.clone(),
        style,
    };
    let mut out = vec![fill(all, all, SCRIM), fill(l.panel, all, PANEL)];
    if !d.alert.title.is_empty() {
        out.push(label(l.title, all, &d.alert.title, bold, style(20.0 * s, WHITE, 0, 1)));
    }
    if let Some(m) = l.message {
        out.push(label(m, all, &d.alert.message, regular, message_style(s)));
    }
    if let Some(f) = l.field {
        field_items(&mut out, d, f, s, all, text);
    }
    for (caption, index, r) in &l.buttons {
        let color = if d.pressed == Some(*index) {
            ACCENT
        } else if d.focus == Some(*index) {
            FOCUS
        } else {
            BUTTON
        };
        out.push(fill(*r, all, color));
        if d.focus == Some(*index) {
            out.push(fill([r[0], r[3] - 2.0 * s, r[2], r[3]], all, ACCENT));
        }
        out.push(label(*r, *r, caption, regular, style(16.0 * s, WHITE, 1, 1)));
    }
    out
}

/// The input line at `f`: white, underlined, the text scrolled so that the caret stays in view.
fn field_items(out: &mut Vec<Item>, d: &Dialog, f: Rect, s: f32, all: Rect, text: &str) {
    let regular = &fonts()[0];
    let fill = |rect: Rect, clip: Rect, argb: u32| Item::Fill { rect, clip, argb, alpha: 1.0 };
    out.push(fill(f, all, WHITE));
    out.push(fill([f[0], f[3] - 2.0 * s, f[2], f[3]], all, ACCENT));
    let (fx0, fx1) = (f[0] + 8.0 * s, f[2] - 8.0 * s);
    let st = field_style(s);
    let sf = Canvas::scaled_for([fx0, f[1], fx1, f[3]], text, &regular.font, &st);
    let units: Vec<u16> = text.encode_utf16().collect();
    let caret = d.caret.map(|c| c.min(units.len()));
    let before = measure(&sf, &String::from_utf16_lossy(&units[..caret.unwrap_or(units.len())]));
    let scroll = (before - (fx1 - fx0) + 2.0 * s).max(0.0);
    let clip = [fx0 - 1.0, f[1], fx1 + 2.0 * s, f[3]];
    if !text.is_empty() {
        let x = fx0 - scroll;
        out.push(Item::Text {
            rect: [x, f[1], x + measure(&sf, text) + 4.0 * s, f[3]],
            clip,
            alpha: 1.0,
            text: text.to_string(),
            font: regular.clone(),
            style: st,
        });
    }
    if caret.is_some() {
        let x = (fx0 - scroll + before).round();
        out.push(fill([x, f[1] + 7.0 * s, x + (2.0 * s).round(), f[3] - 7.0 * s], clip, BLACK));
    }
}

/// A key or controller button for the shown dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogNav {
    /// The focus moves to the next button (right, down, Tab).
    Next,
    /// The focus moves to the previous button (left, up, Shift+Tab).
    Prev,
    /// Press the focused button (Enter, controller A).
    Accept,
    /// Press the cancel (negative) button, if the alert has one (Escape, controller B).
    Cancel,
}

/// The button Enter presses at first: the first of the alert's buttons, else its cancel button.
fn default_button(alert: &Alert) -> Option<i32> {
    alert.buttons.first().or(alert.cancel.as_ref()).map(|b| b.1)
}

fn new_dialog(alert: Alert) -> Dialog {
    let focus = default_button(&alert);
    Dialog { alert, caret: None, pressed: None, focus, pick: None }
}

impl Vm {
    /// Show the dialog of an alert (`AlertView.show`); while another one is shown it waits for its turn.
    pub(crate) fn open_dialog(&self, alert: Alert) {
        let mut st = self.st();
        if st.dialog.as_ref().is_some_and(|d| d.alert.view == alert.view) {
            return;
        }
        if st.dialog.is_some() {
            st.dialog_queue.push(alert);
            return;
        }
        st.dialog = Some(new_dialog(alert));
        drop(st);
        self.ui_changed();
    }

    /// The alert `view` was answered or dismissed: its dialog goes away and the next waiting one is shown.
    pub(crate) fn close_dialog(&self, view: crate::value::Obj) {
        let mut st = self.st();
        st.dialog_queue.retain(|a| a.view != view);
        if st.dialog.as_ref().is_some_and(|d| d.alert.view == view) {
            st.dialog = (!st.dialog_queue.is_empty()).then(|| new_dialog(st.dialog_queue.remove(0)));
            st.ui.generation += 1;
        }
    }

    /// Whether an alert's dialog is shown.
    pub fn dialog_shown(&self) -> bool {
        self.st().dialog.is_some()
    }

    /// The alert whose dialog is shown.
    pub fn dialog_alert(&self) -> Option<Alert> {
        self.st().dialog.as_ref().map(|d| d.alert.clone())
    }

    /// Where the dialog draws the caret (the host's caret of the focused field, UTF-16 units).
    pub fn set_dialog_caret(&self, caret: Option<usize>) {
        let mut st = self.st();
        if let Some(d) = st.dialog.as_mut().filter(|d| d.caret != caret) {
            d.caret = caret;
            st.ui.generation += 1;
        }
    }

    /// A key or controller button for the dialog: moves its focus or picks a button (for
    /// [`Vm::take_dialog_pick`]). False without a dialog.
    pub fn dialog_nav(&self, nav: DialogNav) -> bool {
        let mut st = self.st();
        let Some(d) = st.dialog.as_mut() else { return false };
        // The buttons in the order they are laid out: the cancel button first.
        let order: Vec<i32> = d.alert.cancel.iter().chain(&d.alert.buttons).map(|b| b.1).collect();
        let before = d.focus;
        match nav {
            // The focus stops at the ends (Right on the rightmost button keeps it).
            DialogNav::Next | DialogNav::Prev if !order.is_empty() => {
                let at = d.focus.and_then(|f| order.iter().position(|i| *i == f));
                let to = match (at, nav) {
                    (None, DialogNav::Next) => 0,
                    (None, _) => order.len() - 1,
                    (Some(i), DialogNav::Next) => (i + 1).min(order.len() - 1),
                    (Some(i), _) => i.saturating_sub(1),
                };
                d.focus = Some(order[to]);
            }
            DialogNav::Next | DialogNav::Prev => {}
            DialogNav::Accept => d.pick = d.focus.or_else(|| default_button(&d.alert)),
            DialogNav::Cancel => {
                if let Some(c) = &d.alert.cancel {
                    d.pick = Some(c.1);
                }
            }
        }
        if d.focus != before {
            st.ui.generation += 1;
        }
        true
    }

    /// The alert of the shown dialog and the index of the button the user picked since the last call.
    pub fn take_dialog_pick(&self) -> Option<(Alert, i32)> {
        let mut st = self.st();
        let d = st.dialog.as_mut()?;
        d.pick.take().map(|p| (d.alert.clone(), p))
    }

    /// Whether the shown dialog displays `needle` (ignoring case) in its title, message or a button.
    pub(crate) fn dialog_has_text(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        let st = self.st();
        st.dialog.as_ref().is_some_and(|d| {
            let a = &d.alert;
            [&a.title, &a.message]
                .into_iter()
                .chain(a.cancel.iter().chain(&a.buttons).map(|b| &b.0))
                .any(|t| !t.is_empty() && t.to_lowercase().contains(&needle))
        })
    }

    /// Screen position of the centre of the dialog button showing `needle` (ignoring case; a button that shows
    /// exactly it wins: "Allow" is not "Don't Allow").
    pub(crate) fn dialog_button_pos(&self, needle: &str) -> Option<(f32, f32)> {
        let needle = needle.to_lowercase();
        let (_, l) = self.dialog_layout()?;
        let caption = |b: &&(String, i32, Rect)| b.0.to_lowercase();
        let (_, _, r) = l
            .buttons
            .iter()
            .find(|b| caption(b) == needle)
            .or_else(|| l.buttons.iter().find(|b| caption(b).contains(&needle)))?;
        Some(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0))
    }

    /// The shown dialog and its layout on the current window.
    fn dialog_layout(&self) -> Option<(Dialog, Layout)> {
        let d = self.st().dialog.clone()?;
        let (w, h) = self.screen_size();
        let l = layout((w.max(1) as f32, h.max(1) as f32), &d.alert, &fonts()[0].font);
        Some((d, l))
    }
    /// What the dialog draws (nothing when none is shown).
    pub(super) fn dialog_items(&self) -> Vec<Item> {
        let Some((d, l)) = self.dialog_layout() else { return Vec::new() };
        let text = d.alert.field.map(|f| self.text_of(f)).unwrap_or_default();
        let (w, h) = self.screen_size();
        items(&d, &l, (w.max(1) as f32, h.max(1) as f32), &text)
    }

    /// A touch at window position `(x, y)` while the dialog is shown: `Some(true)`, it takes them all (a press and
    /// release on the same button picks it). `None` without a dialog.
    pub(crate) fn dialog_touch(&self, phase: TouchPhase, x: f32, y: f32) -> Option<bool> {
        let (_, l) = self.dialog_layout()?;
        let under = l.buttons.iter().find(|b| inside(&b.2, x, y)).map(|b| b.1);
        let mut st = self.st();
        let d = st.dialog.as_mut()?;
        let before = d.pressed;
        match phase {
            TouchPhase::Begin => d.pressed = under,
            TouchPhase::Move => {}
            TouchPhase::Cancel => d.pressed = None,
            TouchPhase::End => {
                if let Some(p) = d.pressed.take().filter(|p| under == Some(*p)) {
                    d.pick = Some(p);
                }
            }
        }
        if d.pressed != before {
            st.ui.generation += 1;
        }
        Some(true)
    }
}
