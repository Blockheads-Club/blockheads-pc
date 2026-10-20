//! The UIKit view tree the game builds through `com.apportable.ui.*`, as the host needs it: geometry, hierarchy,
//! visibility and the text of buttons and labels.
//!
//! On Android these are real Java views over the GL surface, so a touch reaches a `Button` or `TextField` before the
//! game's own `Window` sees it. Here the views are plain objects; [`Vm::ui_touch`] hit-tests this tree and plays the
//! part of the Android view system for the controls (button events, text focus), and [`Vm::dump_views`] prints the
//! tree for debugging (the views are not drawn).

use crate::value::{Obj, Value};
use crate::vm::{Throw, Vm};
use std::collections::HashMap;
use std::fmt::Write;

/// One UIKit view.
#[derive(Debug, Clone)]
pub(crate) struct View {
    /// Short class name (`Button`, `Label`, `Window`, ...).
    pub kind: String,
    /// `mObject`: the handle of the native peer.
    pub handle: i32,
    /// `left, top, right, bottom` in the parent's coordinates.
    pub frame: [f32; 4],
    /// Origin of the content (`setBounds`), subtracted from the children's positions.
    pub bounds: [f32; 2],
    pub parent: Option<Obj>,
    pub children: Vec<Obj>,
    pub hidden: bool,
    pub alpha: f32,
    /// `setUserInteractionEnabled(false)` hides the view and its subviews from touches.
    pub interactive: bool,
    pub enabled: bool,
    pub text: String,
    /// `_setBackgroundColor`: ARGB.
    pub bg: Option<u32>,
    /// `setClipsToBounds`.
    pub clips: bool,
    /// `ImageView.setImage`: the bitmap and the size it is meant to be shown at (0 = its own).
    pub image: Option<(Obj, i32, i32)>,
    /// `Button.setBackgroundDrawable(bitmap, state)`.
    pub backgrounds: Vec<(i32, Obj)>,
    /// `Button.setIconDrawable(bitmap, state)`.
    pub icons: Vec<(i32, Obj)>,
    pub highlighted: bool,
    pub style: TextStyle,
}

/// How a label, button caption or text field draws its text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextStyle {
    pub font: Option<Obj>,
    pub size: f32,
    /// ARGB.
    pub color: u32,
    /// `NSTextAlignment`: 0 left, 1 center, 2 right.
    pub align: i32,
    /// 0 = as many as fit.
    pub lines: i32,
    pub min_size: f32,
    pub shadow_color: u32,
    pub shadow_offset: [f32; 2],
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: None,
            size: 17.0,
            color: 0xFF00_0000,
            align: 0,
            lines: 1,
            min_size: 0.0,
            shadow_color: 0,
            shadow_offset: [0.0; 2],
        }
    }
}

impl View {
    /// Heap objects the view keeps alive.
    pub(crate) fn refs(&self) -> impl Iterator<Item = Obj> + '_ {
        let bitmaps = self.backgrounds.iter().chain(&self.icons).map(|b| b.1);
        bitmaps.chain(self.image.map(|i| i.0)).chain(self.style.font)
    }
}

#[derive(Default)]
pub(crate) struct UiState {
    pub views: HashMap<Obj, View>,
    /// Bumped by every change the renderer may draw differently after ([`Vm::ui_changed`]).
    pub generation: u64,
    /// The control that got the touch-down being played (button or text field).
    pub capture: Option<Obj>,
}

/// Phase of a touch handed to [`Vm::ui_touch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Begin,
    Move,
    End,
    Cancel,
}

/// `Control` event masks the native side expects.
const TOUCH_DOWN: i32 = 1;
const TOUCH_UPINSIDE: i32 = 64;
const TOUCH_UPOUTSIDE: i32 = 128;

/// A view found by [`Vm::hit_test`].
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub view: Obj,
    pub kind: String,
    pub handle: i32,
}

fn contains(f: &[f32; 4], origin: (f32, f32), x: f32, y: f32) -> bool {
    let (w, h) = (f[2] - f[0], f[3] - f[1]);
    x >= origin.0 && x < origin.0 + w && y >= origin.1 && y < origin.1 + h
}

impl Vm {
    /// `[left, top, right, bottom]` of a Java `RectF` (zeros for null).
    pub(crate) fn rect_of(&self, r: Option<Obj>) -> [f32; 4] {
        match r {
            Some(r) => ["left", "top", "right", "bottom"].map(|n| self.get_field(r, n).as_float()),
            None => [0.0; 4],
        }
    }

    /// Something the views are drawn from changed (the tree, or the pixels of a bitmap): the renderer looks again.
    pub(crate) fn ui_changed(&self) {
        self.st().ui.generation += 1;
    }

    /// Record a new view.
    pub(crate) fn ui_create(&self, view: Obj, kind: &str, handle: i32, frame: [f32; 4]) {
        self.pin(view);
        self.ui_changed();
        self.st().ui.views.insert(
            view,
            View {
                kind: kind.to_string(),
                handle,
                frame,
                bounds: [0.0; 2],
                parent: None,
                children: Vec::new(),
                hidden: false,
                alpha: 1.0,
                interactive: true,
                enabled: true,
                text: String::new(),
                bg: None,
                clips: false,
                image: None,
                backgrounds: Vec::new(),
                icons: Vec::new(),
                highlighted: false,
                style: TextStyle::default(),
            },
        );
    }

    /// Change a recorded view (unknown views are ignored).
    pub(crate) fn ui_update(&self, view: Obj, f: impl FnOnce(&mut View)) {
        let mut st = self.st();
        if let Some(v) = st.ui.views.get_mut(&view) {
            f(v);
            st.ui.generation += 1;
        }
    }

    /// `parent.addSubview(child)` / `insertSubview(child, at)`; a view has one parent.
    pub(crate) fn ui_attach(&self, parent: Obj, child: Obj, at: Option<usize>) {
        let mut st = self.st();
        st.ui.generation += 1;
        let views = &mut st.ui.views;
        if !views.contains_key(&parent) || !views.contains_key(&child) || parent == child {
            return;
        }
        if let Some(old) = views.get(&child).and_then(|c| c.parent) {
            views.entry(old).and_modify(|o| o.children.retain(|c| *c != child));
        }
        views.entry(parent).and_modify(|p| {
            let at = at.map_or(p.children.len(), |i| i.min(p.children.len()));
            p.children.insert(at, child);
        });
        views.entry(child).and_modify(|c| c.parent = Some(parent));
    }

    /// `parent.removeSubview(child)`.
    pub(crate) fn ui_detach(&self, child: Obj) {
        let mut st = self.st();
        st.ui.generation += 1;
        let views = &mut st.ui.views;
        let Some(parent) = views.get_mut(&child).and_then(|c| c.parent.take()) else { return };
        views.entry(parent).and_modify(|p| p.children.retain(|c| *c != child));
    }

    /// Window-space origin of `view` and whether it hangs below a `Window`, visible all the way.
    fn abs_origin(views: &HashMap<Obj, View>, view: Obj) -> Option<((f32, f32), bool)> {
        let mut chain = vec![view];
        while let Some(p) = views.get(chain.last()?)?.parent {
            if chain.contains(&p) {
                return None;
            }
            chain.push(p);
        }
        let root = views.get(chain.last()?)?;
        let mut origin = (root.frame[0], root.frame[1]);
        let mut shown = !root.hidden && root.alpha > 0.01;
        for pair in chain.windows(2).rev() {
            let (parent, child) = (views.get(&pair[1])?, views.get(&pair[0])?);
            origin.0 += child.frame[0] - parent.bounds[0];
            origin.1 += child.frame[1] - parent.bounds[1];
            shown &= !child.hidden && child.alpha > 0.01;
        }
        Some((origin, shown && root.kind == "Window"))
    }

    /// Frame of `view` in window coordinates `(x, y, w, h)`, with whether it is shown.
    pub fn view_rect(&self, view: Obj) -> Option<([f32; 4], bool)> {
        let st = self.st();
        let (o, shown) = Self::abs_origin(&st.ui.views, view)?;
        let f = st.ui.views.get(&view)?.frame;
        Some(([o.0, o.1, f[2] - f[0], f[3] - f[1]], shown))
    }

    fn hit(views: &HashMap<Obj, View>, v: Obj, origin: (f32, f32), x: f32, y: f32) -> Option<Obj> {
        let info = views.get(&v)?;
        if info.hidden || info.alpha <= 0.01 || !info.interactive {
            return None;
        }
        // Like the renderer, only a clipping view confines its subviews: the game's landscape root view holds the
        // portrait popups, whose buttons lie below its bottom edge.
        let inside = info.kind == "Window" || contains(&info.frame, origin, x, y);
        if !inside && info.clips {
            return None;
        }
        for c in info.children.iter().rev() {
            let Some(ci) = views.get(c) else { continue };
            let co = (origin.0 + ci.frame[0] - info.bounds[0], origin.1 + ci.frame[1] - info.bounds[1]);
            if let Some(h) = Self::hit(views, *c, co, x, y) {
                return Some(h);
            }
        }
        matches!(info.kind.as_str(), "Button" | "TextField" | "TextView")
            .then_some(v)
            .filter(|_| info.enabled && inside)
    }

    /// The topmost enabled control (button, text field) under window position `(x, y)`.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<Hit> {
        let st = self.st();
        let mut windows: Vec<Obj> =
            st.ui.views.iter().filter(|(_, v)| v.kind == "Window" && v.parent.is_none()).map(|(o, _)| *o).collect();
        windows.sort_by_key(|o| std::cmp::Reverse(o.0));
        windows.into_iter().find_map(|w| {
            let root = st.ui.views.get(&w)?;
            let h = Self::hit(&st.ui.views, w, (root.frame[0], root.frame[1]), x, y)?;
            let v = st.ui.views.get(&h)?;
            Some(Hit { view: h, kind: v.kind.clone(), handle: v.handle })
        })
    }

    /// Where the origin of the Java views' space lies in window pixels. The manifest declares the game portrait, so
    /// UIKit lays its views out in a portrait space (768x1024 on the 1024x768 window): the game never receives the
    /// orientation event that would make Apportable turn that space, so the views stay upright and the portrait
    /// space is centred on the window (a popup centred in it is centred on the screen). A portrait window needs no
    /// shift.
    pub fn ui_offset(&self) -> (f32, f32) {
        let (w, h) = self.screen_size();
        if w > h {
            ((w - h) as f32 / 2.0, (h - w) as f32 / 2.0)
        } else {
            (0.0, 0.0)
        }
    }

    /// Window pixels to the coordinates of the Java views (see [`Vm::ui_offset`]).
    pub fn screen_to_ui(&self, x: f32, y: f32) -> (f32, f32) {
        let (ox, oy) = self.ui_offset();
        (x - ox, y - oy)
    }

    /// Inverse of [`Vm::screen_to_ui`].
    pub fn ui_to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        let (ox, oy) = self.ui_offset();
        (x + ox, y + oy)
    }

    /// Views whose text contains `needle` (ignoring case) and that are shown, in creation order.
    fn shown_with_text(&self, needle: &str) -> Vec<Obj> {
        let st = self.st();
        let needle = needle.to_lowercase();
        let mut found: Vec<Obj> = st
            .ui
            .views
            .iter()
            .filter(|(o, v)| {
                !v.text.is_empty()
                    && v.text.to_lowercase().contains(&needle)
                    && Self::abs_origin(&st.ui.views, **o).is_some_and(|(_, shown)| shown)
            })
            .map(|(o, _)| *o)
            .collect();
        found.sort();
        found
    }

    /// True if a shown view displays text containing `needle` (ignoring case).
    pub fn has_text(&self, needle: &str) -> bool {
        self.dialog_has_text(needle) || !self.shown_with_text(needle).is_empty()
    }

    /// Screen position of the centre of the control (the button or field itself, or the one that contains the label)
    /// that shows `needle`: where a user would click. A shown dialog is modal: only its buttons count.
    pub fn control_screen_pos(&self, needle: &str) -> Option<(f32, f32)> {
        if self.dialog_shown() {
            return self.dialog_button_pos(needle);
        }
        let found = *self.shown_with_text(needle).last()?;
        let target = {
            let st = self.st();
            // a label inside a button stands for the button
            let parent = st.ui.views.get(&found).and_then(|i| i.parent);
            parent.filter(|p| st.ui.views.get(p).is_some_and(|i| i.kind == "Button")).unwrap_or(found)
        };
        let ([x, y, w, h], _) = self.view_rect(target)?;
        Some(self.ui_to_screen(x + w / 2.0, y + h / 2.0))
    }

    /// Play the Android view system for the controls: a touch that starts on a button or text field belongs to it
    /// (returns `true`: do not pass it to the game's window). A button gets `Control.event(handle, mask)` for the
    /// touch down and for the touch up (inside or outside); a text field gets the keyboard focus. Everything else
    /// returns `false`.
    pub fn ui_touch(&self, cpu: &mut bhpc_cpu::Cpu, phase: TouchPhase, x: f32, y: f32) -> Result<bool, Throw> {
        self.note_user_input();
        // An alert's input dialog is modal.
        if let Some(taken) = self.dialog_touch(phase, x, y) {
            return Ok(taken);
        }
        let (x, y) = self.screen_to_ui(x, y);
        match phase {
            TouchPhase::Begin => {
                let hit = self.hit_test(x, y);
                self.st().ui.capture = hit.as_ref().map(|h| h.view);
                if let Some(h) = hit.as_ref().filter(|h| h.kind == "Button") {
                    self.call_method(cpu, h.view, "event", "(II)V", &[Value::Int(h.handle), Value::Int(TOUCH_DOWN)])?;
                }
                Ok(hit.is_some())
            }
            TouchPhase::Move => Ok(self.st().ui.capture.is_some()),
            TouchPhase::Cancel => Ok(self.st().ui.capture.take().is_some()),
            TouchPhase::End => {
                let Some(v) = self.st().ui.capture.take() else { return Ok(false) };
                let inside = self.hit_test(x, y).is_some_and(|h| h.view == v);
                let (kind, handle) = self.st().ui.views.get(&v).map(|i| (i.kind.clone(), i.handle)).unwrap_or_default();
                if kind == "Button" {
                    let ev = if inside { TOUCH_UPINSIDE } else { TOUCH_UPOUTSIDE };
                    self.call_method(cpu, v, "event", "(II)V", &[Value::Int(handle), Value::Int(ev)])?;
                } else if inside {
                    self.set_text_focus(Some(v));
                }
                Ok(true)
            }
        }
    }

    /// `WxH` of a bitmap and how many of its pixels are not fully transparent (for the dump).
    pub(crate) fn bitmap_summary(&self, b: Obj) -> String {
        let Some((w, h, ptr)) = crate::jlib::bitmap_info(self, b) else { return "none".into() };
        let px = self.mem().read_bytes(ptr, w * h * 4).unwrap_or_default();
        format!("{w}x{h}:{}", px.chunks(4).filter(|p| p[3] != 0).count())
    }

    /// The view tree as indented text: kind, handle, window-space rectangle, flags, text.
    pub fn dump_views(&self) -> String {
        let views = self.st().ui.views.clone();
        let mut roots: Vec<Obj> = views.iter().filter(|(_, v)| v.parent.is_none()).map(|(o, _)| *o).collect();
        roots.sort();
        let mut out = String::new();
        for r in roots {
            self.dump(&views, r, 0, (0.0, 0.0), &mut out);
        }
        out
    }

    fn dump(&self, views: &HashMap<Obj, View>, v: Obj, depth: usize, parent_origin: (f32, f32), out: &mut String) {
        let Some(i) = views.get(&v) else { return };
        let o = (parent_origin.0 + i.frame[0], parent_origin.1 + i.frame[1]);
        let _ = write!(
            out,
            "{:indent$}{}#{} @{} [{:.0},{:.0} {:.0}x{:.0}]",
            "",
            i.kind,
            i.handle,
            v.0,
            o.0,
            o.1,
            i.frame[2] - i.frame[0],
            i.frame[3] - i.frame[1],
            indent = depth * 2
        );
        for (flag, on) in [("hidden", i.hidden), ("noinput", !i.interactive), ("disabled", !i.enabled)] {
            if on {
                let _ = write!(out, " {flag}");
            }
        }
        if i.alpha < 1.0 {
            let _ = write!(out, " alpha={:.2}", i.alpha);
        }
        if let Some(bg) = i.bg {
            let _ = write!(out, " bg=#{bg:08x}");
        }
        if i.clips {
            out.push_str(" clip");
        }
        for (what, b) in i.image.iter().map(|b| ("img", b.0)).chain(i.backgrounds.iter().map(|b| ("bgimg", b.1))) {
            let _ = write!(out, " {what}={}", self.bitmap_summary(b));
        }
        if !i.text.is_empty() {
            let f = i.style.font.map(|t| crate::jlib::face_of(self, t)).unwrap_or_default();
            let st = &i.style;
            let _ = write!(
                out,
                " font={:?}/{:?}/{} {}pt color=#{:08x} align={} lines={} {:?}",
                f.path, f.name, f.style, st.size, st.color, st.align, st.lines, i.text
            );
        }
        out.push('\n');
        for c in &i.children {
            let co = (o.0 - i.bounds[0], o.1 - i.bounds[1]);
            self.dump(views, *c, depth + 1, co, out);
        }
    }
}
