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

/// The landscape window as the views see it, for [`Landscape::shift`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Landscape {
    size: (f32, f32),
    offset: (f32, f32),
}

impl Landscape {
    /// How far a child's origin moves back when it is a landscape layout inside the portrait space. The window's
    /// landscape root (its full-screen child) holds both: the game's popups, laid out for the portrait space and
    /// centred across it, which [`Vm::ui_offset`] centres in the window, and the chat, laid out for the landscape
    /// screen (full-screen on a phone, a panel at the left on a tablet), whose (0, 0) is the window's corner.
    /// `parent` is the child's parent; `views` finds the parent's own parent.
    pub(crate) fn shift(
        land: Option<Landscape>,
        views: &HashMap<Obj, View>,
        parent: &View,
        child: &View,
    ) -> (f32, f32) {
        let Some(l) = land else { return (0.0, 0.0) };
        let root = parent.frame == [0.0, 0.0, l.size.0, l.size.1]
            && parent.parent.and_then(|g| views.get(&g)).is_some_and(|g| g.kind == "Window");
        let portrait_centre = l.size.0.min(l.size.1) / 2.0;
        let centred = ((child.frame[0] + child.frame[2]) / 2.0 - portrait_centre).abs() < 1.0;
        if root && !centred {
            (-l.offset.0, -l.offset.1)
        } else {
            (0.0, 0.0)
        }
    }
}

/// Space between the desktop chat's parts.
const CHAT_PAD: f32 = 4.0;
/// Height of the desktop chat's input line (the game's is two lines tall).
const CHAT_INPUT_H: f32 = 44.0;
/// Size of the sender's picture in a desktop chat row, and the height of the date line above the message.
const CHAT_AVATAR: f32 = 36.0;
const CHAT_DATE_H: f32 = 16.0;
/// Narrowest SEND button (the game's can be too narrow for its caption).
const CHAT_SEND_MIN_W: f32 = 76.0;
/// The desktop chat's text is set smaller than the phone's.
pub(crate) const CHAT_TEXT_SCALE: f32 = 0.8;

/// The shown chat panel's header buttons: CLOSE (left-most), the right-most one (SEND while composing, NEW in the
/// list), whether it is composing, and its input line.
pub(crate) struct ChatButtons {
    close: Obj,
    right: Obj,
    composing: bool,
    text: Option<Obj>,
}

/// The parts of the game's chat panel (a view with a text view and a scroll view among its children) the desktop
/// layout places, and the size of the box it draws (its background picture, else the panel).
struct ChatPanel<'a> {
    size: (f32, f32),
    header: Option<&'a View>,
    /// The input line; the game takes it out of the panel while it lists the messages.
    text: Option<&'a View>,
    scroll: &'a View,
    /// The header's right-most shown button (SEND) while the input line is shown (the game is composing).
    send: Option<&'a View>,
    /// The input line is shown: the panel is composing a message (otherwise it lists them, with NEW in the header).
    composing: bool,
}

fn size_of(f: &[f32; 4]) -> (f32, f32) {
    (f[2] - f[0], f[3] - f[1])
}

fn chat_panel<'a>(views: &'a HashMap<Obj, View>, panel: &View) -> Option<ChatPanel<'a>> {
    let kids = || panel.children.iter().filter_map(|c| views.get(c));
    // The input line: the game sometimes keeps an empty, flat one besides it.
    let tall = |v: &&View| v.frame[3] - v.frame[1];
    let text = kids().filter(|v| v.kind == "TextView").max_by(|a, b| tall(a).total_cmp(&tall(b)));
    let scroll = kids().find(|v| v.kind == "ScrollView")?;
    let at_corner = |v: &&View| v.frame[0] == 0.0 && v.frame[1] == 0.0;
    let bg = kids().filter(at_corner).find(|v| v.kind == "ImageView");
    let size = size_of(&bg.unwrap_or(panel).frame);
    let buttons = |h: &View| -> Vec<&'a View> {
        h.children.iter().filter_map(|c| views.get(c)).filter(|v| v.kind == "Button").collect()
    };
    let header = kids().filter(at_corner).find(|v| v.kind == "View" && !buttons(v).is_empty());
    // Without its input line the panel is known by its title.
    let titled =
        |h: &View| h.children.iter().filter_map(|c| views.get(c)).any(|v| v.kind == "Label" && v.text == "CHAT");
    if text.is_none() && !header.is_some_and(titled) {
        return None;
    }
    let composing = text.is_some_and(|t| !t.hidden);
    let send = header.map(buttons).filter(|_| composing).and_then(|b| {
        let shown: Vec<&View> = b.into_iter().filter(|v| !v.hidden).collect();
        (shown.len() > 1).then(|| shown.into_iter().max_by(|x, y| x.frame[0].total_cmp(&y.frame[0])))?
    });
    Some(ChatPanel { size, header, text, scroll, send, composing })
}

impl ChatPanel<'_> {
    /// Height of the input line.
    fn input_h(&self) -> f32 {
        self.text.map_or(0.0, |t| (t.frame[3] - t.frame[1]).min(CHAT_INPUT_H))
    }

    /// Size of SEND (wide enough for its caption, as tall as fits the input line).
    fn send_size(&self, b: &View) -> (f32, f32) {
        let (cw, ch) = size_of(&b.frame);
        (cw.max(CHAT_SEND_MIN_W), ch.min(self.input_h() - 2.0 * CHAT_PAD))
    }
}

/// `parent` is the content of a message row of a chat panel's list (sender picture, message, date).
fn chat_row_content(views: &HashMap<Obj, View>, parent: &View) -> bool {
    let up = |v: &View| v.parent.and_then(|p| views.get(&p));
    let Some(scroll) = up(parent).and_then(up).filter(|s| s.kind == "ScrollView") else { return false };
    up(scroll).and_then(|p| chat_panel(views, p)).is_some_and(|c| std::ptr::eq(c.scroll, scroll))
}

/// How much smaller the desktop chat sets the text of `child` of `parent` (its messages and input line): 1 elsewhere.
pub(crate) fn text_scale(views: &HashMap<Obj, View>, parent: &View, child: &View) -> f32 {
    let input = || chat_panel(views, parent).is_some_and(|c| c.text.is_some_and(|t| std::ptr::eq(t, child)));
    let text = matches!(child.kind.as_str(), "TextView" | "Label");
    if text && (input() || chat_row_content(views, parent)) {
        CHAT_TEXT_SCALE
    } else {
        1.0
    }
}

/// Where the desktop chat puts `child` of `parent` (`left, top, right, bottom` in the parent's content), when it is
/// a part of the game's chat panel. The game draws the panel as on a phone: header (CLOSE, CHAT, SEND), the input
/// line under it, then the messages with the newest at the top. On the desktop the header spans the box, the
/// messages follow it from the oldest to the newest, and the input line sits at the bottom with SEND at its right.
/// Only drawing and touches move; the game's own layout is unchanged.
fn chat_frame(views: &HashMap<Obj, View>, parent: &View, child: &View) -> Option<[f32; 4]> {
    let grand = || parent.parent.and_then(|p| views.get(&p));
    if let Some(c) = chat_panel(views, parent) {
        let (w, h) = c.size;
        // Without the input line the messages reach the bottom.
        let row = if c.composing { c.input_h() } else { 0.0 };
        let send_w = c.send.map_or(0.0, |b| c.send_size(b).0 + CHAT_PAD);
        let head = c.header.map(|v| v.frame[3]).or(c.text.map(|t| t.frame[1])).unwrap_or(0.0);
        if c.header.is_some_and(|v| std::ptr::eq(v, child)) {
            return Some([0.0, 0.0, w, head]);
        }
        if std::ptr::eq(child, c.scroll) {
            return Some([CHAT_PAD, head + CHAT_PAD, w - CHAT_PAD, h - row - CHAT_PAD]);
        }
        // The input line and what lies in its band (the picture behind it).
        if c.text.is_some_and(|t| child.frame[1] >= t.frame[1] && child.frame[1] < t.frame[3]) {
            // The text starts a little in from the box's edge.
            let left = if child.kind == "TextView" { CHAT_PAD } else { 0.0 };
            // It reaches up to the list: the panel's picture under the gap shows the phone's layout.
            return Some([left, h - row - CHAT_PAD, w - send_w - CHAT_PAD, h]);
        }
        return None;
    }
    // The header's own parts: the title and the line under it span the box, SEND goes next to the input line.
    if let Some(c) =
        grand().and_then(|g| chat_panel(views, g)).filter(|c| c.header.is_some_and(|v| std::ptr::eq(v, parent)))
    {
        let (w, h) = c.size;
        let ch = child.frame[3] - child.frame[1];
        if let Some(b) = c.send.filter(|b| std::ptr::eq(*b, child)) {
            // SEND is only taken while the input line is shown.
            let row = c.input_h();
            let (bw, bh) = c.send_size(b);
            let top = h - row + (row - bh) / 2.0 - parent.frame[1];
            return Some([w - CHAT_PAD - bw, top, w - CHAT_PAD, top + bh]);
        }
        if child.kind == "Label" || (child.kind == "View" && ch <= 4.0) {
            return Some([0.0, child.frame[1], w, child.frame[3]]);
        }
        return None;
    }
    // SEND's caption fills the (resized) button, centred.
    if let Some(c) = grand()
        .and_then(|h| h.parent.and_then(|p| views.get(&p)))
        .and_then(|p| chat_panel(views, p))
        .filter(|c| c.send.is_some_and(|b| std::ptr::eq(b, parent)))
    {
        let (bw, bh) = c.send_size(parent);
        return (child.kind == "Label").then_some([0.0, 0.0, bw, bh]);
    }
    // A message row's parts: the sender's picture at the left, the date small at the top right, the message under it.
    if chat_row_content(views, parent) {
        let (w, h) = size_of(&parent.frame);
        let left = 2.0 * CHAT_PAD + CHAT_AVATAR;
        return match child.kind.as_str() {
            "ImageView" => Some([CHAT_PAD, CHAT_PAD, CHAT_PAD + CHAT_AVATAR, CHAT_PAD + CHAT_AVATAR]),
            "Label" => Some([left, 2.0, w - CHAT_PAD, 2.0 + CHAT_DATE_H]),
            "TextView" => Some([left, CHAT_DATE_H, w - CHAT_PAD, h - 2.0]),
            _ => None,
        };
    }
    // A message row: mirrored in the shown part of the list, so the newest is at the bottom.
    let panel = grand().and_then(|g| chat_panel(views, g)).filter(|c| std::ptr::eq(c.scroll, parent))?;
    let list = chat_frame(views, grand()?, panel.scroll).unwrap_or(parent.frame);
    let (top, ch) = (child.frame[1] - parent.bounds[1], child.frame[3] - child.frame[1]);
    // A row the game left under a later one (same place, as tall or taller) stays hidden, as it is on the phone.
    let later = parent.children.iter().skip_while(|c| views.get(c).is_none_or(|v| !std::ptr::eq(v, child))).skip(1);
    let covered = later.filter_map(|c| views.get(c)).any(|v| {
        !v.hidden && v.frame[1] == child.frame[1] && v.frame[3] >= child.frame[3] && v.frame[0] <= child.frame[0]
    });
    if covered {
        return Some([child.frame[0], -ch, child.frame[2], 0.0]);
    }
    let mirrored = (list[3] - list[1]) - top - ch + parent.bounds[1];
    Some([child.frame[0], mirrored, child.frame[2], mirrored + ch])
}

/// Where `child` lies in `parent`'s content (`left, top, right, bottom`): its frame, placed by the desktop chat
/// layout ([`chat_frame`]) and moved back when it is a landscape layout ([`Landscape::shift`]).
pub(crate) fn placed(land: Option<Landscape>, views: &HashMap<Obj, View>, parent: &View, child: &View) -> [f32; 4] {
    let f = chat_frame(views, parent, child).unwrap_or(child.frame);
    let (dx, dy) = Landscape::shift(land, views, parent, child);
    [f[0] + dx, f[1] + dy, f[2] + dx, f[3] + dy]
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

    /// Short class name of a recorded view.
    pub fn view_kind(&self, view: Obj) -> Option<String> {
        self.st().ui.views.get(&view).map(|v| v.kind.clone())
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
    fn abs_origin(views: &HashMap<Obj, View>, view: Obj, land: Option<Landscape>) -> Option<((f32, f32), bool)> {
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
            let f = placed(land, views, parent, child);
            origin.0 += f[0] - parent.bounds[0];
            origin.1 += f[1] - parent.bounds[1];
            shown &= !child.hidden && child.alpha > 0.01;
        }
        Some((origin, shown && root.kind == "Window"))
    }

    /// Frame of `view` in window coordinates `(x, y, w, h)`, with whether it is shown.
    pub fn view_rect(&self, view: Obj) -> Option<([f32; 4], bool)> {
        let st = self.st();
        let land = self.landscape();
        let (o, shown) = Self::abs_origin(&st.ui.views, view, land)?;
        let v = st.ui.views.get(&view)?;
        let f = v.parent.and_then(|p| st.ui.views.get(&p)).map_or(v.frame, |p| placed(land, &st.ui.views, p, v));
        Some(([o.0, o.1, f[2] - f[0], f[3] - f[1]], shown))
    }

    fn hit(
        views: &HashMap<Obj, View>,
        v: Obj,
        frame: [f32; 4],
        origin: (f32, f32),
        land: Option<Landscape>,
        x: f32,
        y: f32,
    ) -> Option<Obj> {
        let info = views.get(&v)?;
        if info.hidden || info.alpha <= 0.01 || !info.interactive {
            return None;
        }
        // Like the renderer, only a clipping view confines its subviews: the game's landscape root view holds the
        // portrait popups, whose buttons lie below its bottom edge.
        let inside = info.kind == "Window" || contains(&frame, origin, x, y);
        if !inside && info.clips {
            return None;
        }
        for c in info.children.iter().rev() {
            let Some(ci) = views.get(c) else { continue };
            let f = placed(land, views, info, ci);
            let co = (origin.0 + f[0] - info.bounds[0], origin.1 + f[1] - info.bounds[1]);
            if let Some(h) = Self::hit(views, *c, f, co, land, x, y) {
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
        let land = self.landscape();
        windows.into_iter().find_map(|w| {
            let root = st.ui.views.get(&w)?;
            let h = Self::hit(&st.ui.views, w, root.frame, (root.frame[0], root.frame[1]), land, x, y)?;
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

    /// The landscape window, when the views' portrait space is shifted in it (see [`Landscape::shift`]).
    pub(crate) fn landscape(&self) -> Option<Landscape> {
        let (w, h) = self.screen_size();
        (w > h).then(|| Landscape { size: (w as f32, h as f32), offset: self.ui_offset() })
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
        let land = self.landscape();
        let mut found: Vec<Obj> = st
            .ui
            .views
            .iter()
            .filter(|(o, v)| {
                !v.text.is_empty()
                    && v.text.to_lowercase().contains(&needle)
                    && Self::abs_origin(&st.ui.views, **o, land).is_some_and(|(_, shown)| shown)
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
                    let before = self.chat_buttons();
                    self.call_method(cpu, v, "event", "(II)V", &[Value::Int(handle), Value::Int(ev)])?;
                    if inside {
                        self.after_chat_press(cpu, v, before)?;
                    }
                } else if inside {
                    self.set_text_focus(Some(v));
                }
                Ok(true)
            }
        }
    }

    /// The button the return key stands for in a text view: the right-most shown button below the view's parent (the
    /// chat's SEND, which sits opposite its CLOSE).
    pub fn return_button(&self, field: Obj) -> Option<Obj> {
        let st = self.st();
        let views = &st.ui.views;
        let mut found: Option<(f32, Obj)> = None;
        let mut todo = vec![views.get(&field)?.parent?];
        while let Some(v) = todo.pop() {
            let Some(info) = views.get(&v) else { continue };
            if info.hidden || info.alpha <= 0.01 {
                continue;
            }
            todo.extend(info.children.iter().copied());
            if info.kind == "Button" && info.enabled {
                let x = Self::abs_origin(views, v, self.landscape()).map_or(0.0, |o| o.0 .0);
                if found.is_none_or(|f| x > f.0) {
                    found = Some((x, v));
                }
            }
        }
        found.map(|f| f.1)
    }

    /// The CLOSE button of the shown chat panel (the left-most button of its header) and whether the panel is
    /// composing a message: then CLOSE first goes back to the list, and a second press closes the panel.
    pub fn chat_close_button(&self) -> Option<(Obj, bool)> {
        self.chat_buttons().map(|b| (b.close, b.composing))
    }

    /// The shown chat panel's header buttons and input line.
    pub(crate) fn chat_buttons(&self) -> Option<ChatButtons> {
        let st = self.st();
        let views = &st.ui.views;
        let land = self.landscape();
        views.iter().find_map(|(o, v)| {
            let panel = chat_panel(views, v)?;
            let header = panel.header?;
            Self::abs_origin(views, *o, land).filter(|(_, shown)| *shown)?;
            let shown: Vec<(Obj, &View)> = header
                .children
                .iter()
                .filter_map(|b| views.get(b).filter(|v| v.kind == "Button" && !v.hidden).map(|v| (*b, v)))
                .collect();
            let x = |a: &&(Obj, &View), b: &&(Obj, &View)| a.1.frame[0].total_cmp(&b.1.frame[0]);
            let (close, right) = (shown.iter().min_by(x)?.0, shown.iter().max_by(x)?.0);
            let text = v
                .children
                .iter()
                .copied()
                .find(|c| panel.text.is_some_and(|t| views.get(c).is_some_and(|i| std::ptr::eq(i, t))));
            Some(ChatButtons { close, right, composing: panel.composing, text })
        })
    }

    /// Press a chat button as the player does: the desktop chat then stays open for the next message after a send
    /// (the game goes back to the list: NEW is pressed and the input line takes the keyboard), and CLOSE closes the
    /// whole chat (the game first goes back to the list: CLOSE is pressed again). Any other button: a plain press.
    pub fn press_chat_button(&self, cpu: &mut bhpc_cpu::Cpu, button: Obj) -> Result<(), Throw> {
        let before = self.chat_buttons();
        self.press_button(cpu, button)?;
        self.after_chat_press(cpu, button, before)
    }

    /// The desktop chat's follow-up to a press of `button` while it was composing (see [`Vm::press_chat_button`]).
    pub(crate) fn after_chat_press(
        &self,
        cpu: &mut bhpc_cpu::Cpu,
        button: Obj,
        before: Option<ChatButtons>,
    ) -> Result<(), Throw> {
        let Some(b) = before.filter(|b| b.composing) else { return Ok(()) };
        let Some(now) = self.chat_buttons().filter(|n| !n.composing) else { return Ok(()) };
        if button == b.close {
            return self.press_button(cpu, now.close);
        }
        if button == b.right {
            self.press_button(cpu, now.right)?;
            if let Some(t) = self.chat_buttons().and_then(|n| n.text) {
                self.set_text_focus(Some(t));
            }
        }
        Ok(())
    }

    /// Tap a button as the view system does for a touch inside it: `TouchDown`, then `TouchUpInside`.
    pub fn press_button(&self, cpu: &mut bhpc_cpu::Cpu, button: Obj) -> Result<(), Throw> {
        let handle = self.st().ui.views.get(&button).map_or(0, |v| v.handle);
        for mask in [TOUCH_DOWN, TOUCH_UPINSIDE] {
            self.call_method(cpu, button, "event", "(II)V", &[Value::Int(handle), Value::Int(mask)])?;
        }
        Ok(())
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
