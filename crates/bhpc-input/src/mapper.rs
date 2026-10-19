//! The stateful host-to-guest mapping.

use crate::event::{
    Bindings, EventSink, GuestEvent, HostAction, HostEvent, HostKey, Input, Lifecycle, Mods, MouseButton,
};
use crate::keymap;
use crate::pad::{PadConfig, PadState};
use crate::text::TextModel;

/// How the game's content surface sits in the window: scaled uniformly to fit and centred (letterboxed).
/// The window is in the unit mouse events use (logical window pixels), the content in game pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Size of the coordinate space the guest sees. `None` passes window pixels through unscaled.
    pub content: Option<(f32, f32)>,
    pub window: (u32, u32),
}

impl Geometry {
    /// Scale (window units per content pixel) and top-left offset of the letterboxed viewport; `None` without a
    /// content size or with an empty window or content.
    pub fn viewport(&self) -> Option<(f32, (f32, f32))> {
        let (cw, ch) = self.content?;
        let (ww, wh) = (self.window.0 as f32, self.window.1 as f32);
        if ww <= 0.0 || wh <= 0.0 || cw <= 0.0 || ch <= 0.0 {
            return None;
        }
        let s = (ww / cw).min(wh / ch);
        Some((s, ((ww - cw * s) / 2.0, (wh - ch * s) / 2.0)))
    }

    /// Window position to content pixels (outside the viewport the result lies outside the content).
    pub fn to_content(&self, x: f32, y: f32) -> Pt {
        match self.viewport() {
            Some((s, (ox, oy))) => ((x - ox) / s, (y - oy) / s),
            None => (x, y),
        }
    }

    /// Content pixels to window position (inverse of [`Geometry::to_content`]).
    pub fn to_window(&self, x: f32, y: f32) -> Pt {
        match self.viewport() {
            Some((s, (ox, oy))) => (x * s + ox, y * s + oy),
            None => (x, y),
        }
    }
}

/// Tunables of the mapping. Distances are in content units (see `content`).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Size of the coordinate space the guest sees (the game's content size). `None` passes window pixels through
    /// unscaled. The default is a 1024x768 surface; the game announces its real size (`InputMapper::set_content`).
    pub content: Option<(f32, f32)>,
    /// Window size in logical pixels, the unit of mouse coordinates (updated by `Resized`).
    pub window: (u32, u32),
    /// Distance between the two fingers of a right-drag pan.
    pub pan_spacing: f32,
    /// Finger distance from the centre when a pinch starts (half the finger spacing).
    pub pinch_start_radius: f32,
    /// Radius change per wheel notch.
    pub pinch_step: f32,
    pub pinch_min_radius: f32,
    pub pinch_max_radius: f32,
    /// A wheel pinch ends when no wheel event arrived for this long.
    pub pinch_idle_ms: i64,
    /// What the buttons, wheel and keys do.
    pub bindings: Bindings,
    /// Game controller tunables.
    pub pad: PadConfig,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            content: Some((1024.0, 768.0)),
            window: (1024, 768),
            pan_spacing: 80.0,
            pinch_start_radius: 100.0,
            pinch_step: 20.0,
            pinch_min_radius: 20.0,
            pinch_max_radius: 400.0,
            pinch_idle_ms: 150,
            bindings: Bindings::default(),
            pad: PadConfig::default(),
        }
    }
}

type Pt = (f32, f32);

#[derive(Debug, Clone, PartialEq)]
enum Gesture {
    None,
    /// One finger, pointer 0.
    Touch,
    /// Two fingers `spacing` apart, centred on the cursor.
    Pan,
    /// Two fingers `radius` either side of `center`; `anchor` is `Some` for Ctrl+drag (the radius
    /// follows the cursor), `None` for the wheel (which ends on idle).
    Pinch {
        center: Pt,
        radius: f32,
        anchor: Option<(Pt, f32)>,
        last_wheel_ms: i64,
    },
}

struct TextFocus {
    field: i32,
    model: TextModel,
}

/// Translates [`HostEvent`]s into [`GuestEvent`]s. One instance per window.
pub struct InputMapper {
    cfg: Config,
    gesture: Gesture,
    /// Button that owns the active gesture (`None` for the wheel pinch).
    owner: Option<MouseButton>,
    /// Last cursor position in content units (what the guest last saw).
    pos: Pt,
    text: Option<TextFocus>,
    pad: PadState,
}

impl InputMapper {
    pub fn new(cfg: Config) -> InputMapper {
        let bounds = cfg.content.unwrap_or((cfg.window.0 as f32, cfg.window.1 as f32));
        let pad = PadState::new(cfg.pad, bounds);
        InputMapper { cfg, gesture: Gesture::None, owner: None, pos: (0.0, 0.0), text: None, pad }
    }

    /// The controller state (cursor, game knowledge for walking and snapping).
    pub fn pad(&self) -> &PadState {
        &self.pad
    }

    /// The controller state, to tell it where the game's D-pad and tiles are.
    pub fn pad_mut(&mut self) -> &mut PadState {
        &mut self.pad
    }

    /// True while a touch, pan or pinch is in progress.
    pub fn gesture_active(&self) -> bool {
        self.gesture != Gesture::None
    }

    /// Where the content sits in the window right now.
    pub fn geometry(&self) -> Geometry {
        Geometry { content: self.cfg.content, window: self.cfg.window }
    }

    /// Window pixels to content units (through the letterboxed viewport).
    pub fn to_content(&self, x: f32, y: f32) -> Pt {
        self.geometry().to_content(x, y)
    }

    /// The game announced a new content size (`VerdeActivity.setContentSize`).
    pub fn set_content(&mut self, width: f32, height: f32) {
        self.cfg.content = Some((width, height));
        self.pad.set_bounds((width, height));
    }

    /// Contents of the focused text field as the host tracks it, if one has focus.
    pub fn text(&self) -> Option<String> {
        self.text.as_ref().map(|t| t.model.text())
    }

    /// A guest text field gained (`Some((handle, current text))`) or lost (`None`) focus. Emits the
    /// matching start/stop-text-input host action when the focus state changes.
    pub fn set_text_focus(&mut self, focus: Option<(i32, &str)>, sink: &mut dyn EventSink) {
        let had = self.text.is_some();
        self.text = focus.map(|(field, s)| TextFocus { field, model: TextModel::new(s) });
        match (had, self.text.is_some()) {
            (false, true) => sink.host_action(HostAction::StartTextInput),
            (true, false) => sink.host_action(HostAction::StopTextInput),
            _ => {}
        }
    }

    /// Advance time: ends a wheel pinch that has been idle. Call about once per frame.
    pub fn tick(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        if let Gesture::Pinch { anchor: None, last_wheel_ms, .. } = self.gesture {
            if now_ms - last_wheel_ms >= self.cfg.pinch_idle_ms {
                self.end_gesture(now_ms, sink);
            }
        }
        self.pad.tick(now_ms, sink);
    }

    /// Process one host event.
    pub fn handle(&mut self, ev: HostEvent, now_ms: i64, sink: &mut dyn EventSink) {
        match ev {
            HostEvent::MouseMove { x, y } => {
                self.pad.mouse_moved();
                let p = self.to_content(x, y);
                self.move_to(p, now_ms, sink);
            }
            HostEvent::MouseButton { button, pressed: true, x, y, mods } => {
                self.press(button, self.to_content(x, y), mods, now_ms, sink)
            }
            HostEvent::MouseButton { button, pressed: false, x, y, .. } => {
                if self.owner == Some(button) {
                    let p = self.to_content(x, y);
                    self.move_to(p, now_ms, sink);
                    self.end_gesture(now_ms, sink);
                }
            }
            HostEvent::Wheel { dy, x, y, mods } => self.wheel(dy, self.to_content(x, y), mods, now_ms, sink),
            HostEvent::Key { key, pressed, repeat, mods } => self.key(key, pressed, repeat, mods, now_ms, sink),
            HostEvent::TextInput(s) => self.insert_text(&s, sink),
            // A desktop game keeps running unfocused. Android's resign-active would pause the game (it shows its pause
            // menu), and the focus flips of a compositor around every click would trigger that, so focus changes
            // only drop a touch in progress. Only a minimised window goes to the background.
            HostEvent::FocusGained => self.pad.set_focused(true),
            HostEvent::FocusLost => {
                self.cancel_gesture(now_ms, sink);
                self.pad.release(now_ms, sink);
                self.pad.set_focused(false);
            }
            HostEvent::Minimized => {
                self.cancel_gesture(now_ms, sink);
                self.pad.release(now_ms, sink);
                sink.deliver(&GuestEvent::Lifecycle(Lifecycle::WillResignActive));
                sink.deliver(&GuestEvent::Lifecycle(Lifecycle::DidEnterBackground));
            }
            HostEvent::Restored => {
                sink.deliver(&GuestEvent::Lifecycle(Lifecycle::WillEnterForeground));
                sink.deliver(&GuestEvent::Lifecycle(Lifecycle::DidBecomeActive));
            }
            HostEvent::Resized { width, height, logical_width, logical_height } => {
                self.cfg.window = (logical_width, logical_height);
                sink.deliver(&GuestEvent::SurfaceChanged { width, height });
            }
            HostEvent::Quit => sink.host_action(HostAction::Quit),
            HostEvent::Pad(e) => {
                let busy = self.gesture != Gesture::None;
                self.pad.handle(e, now_ms, busy, sink);
            }
        }
    }

    // ---------- gestures ----------

    /// Current finger positions, pointer ids 0 and (for two-finger gestures) 1.
    fn fingers(&self) -> (Vec<i32>, Vec<f32>, Vec<f32>) {
        let (x, y) = self.pos;
        match &self.gesture {
            Gesture::None => (vec![], vec![], vec![]),
            Gesture::Touch => (vec![0], vec![x], vec![y]),
            Gesture::Pan => {
                let h = self.cfg.pan_spacing / 2.0;
                (vec![0, 1], vec![x - h, x + h], vec![y, y])
            }
            Gesture::Pinch { center, radius, .. } => {
                (vec![0, 1], vec![center.0 - radius, center.0 + radius], vec![center.1, center.1])
            }
        }
    }

    fn begin_gesture(&mut self, g: Gesture, owner: Option<MouseButton>, now_ms: i64, sink: &mut dyn EventSink) {
        self.gesture = g;
        self.owner = owner;
        let (ids, xs, ys) = self.fingers();
        for i in 0..ids.len() {
            sink.deliver(&GuestEvent::TouchBegin { id: ids[i], x: xs[i], y: ys[i], time_ms: now_ms });
        }
    }

    fn end_gesture(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        let (ids, xs, ys) = self.fingers();
        for i in 0..ids.len() {
            sink.deliver(&GuestEvent::TouchEnd { id: ids[i], x: xs[i], y: ys[i], time_ms: now_ms });
        }
        self.gesture = Gesture::None;
        self.owner = None;
    }

    fn cancel_gesture(&mut self, now_ms: i64, sink: &mut dyn EventSink) {
        let (ids, xs, ys) = self.fingers();
        if !ids.is_empty() {
            sink.deliver(&GuestEvent::TouchCancel { ids, xs, ys, time_ms: now_ms });
        }
        self.gesture = Gesture::None;
        self.owner = None;
    }

    fn send_move(&self, now_ms: i64, sink: &mut dyn EventSink) {
        let (ids, xs, ys) = self.fingers();
        sink.deliver(&GuestEvent::TouchMove { ids, xs, ys, time_ms: now_ms });
    }

    fn press(&mut self, button: MouseButton, p: Pt, mods: Mods, now_ms: i64, sink: &mut dyn EventSink) {
        // A wheel pinch gives way to a click.
        if matches!(self.gesture, Gesture::Pinch { anchor: None, .. }) {
            self.end_gesture(now_ms, sink);
        }
        // Pointer 0 is taken by another gesture or by the controller's touch.
        if self.gesture != Gesture::None || self.pad.touching() {
            return;
        }
        self.pos = p;
        let b = &self.cfg.bindings;
        let input = Input::Mouse(button);
        // The most specific binding that fires wins; on a tie the order pinch, touch, pan decides.
        let picked = [(0, &b.pinch), (1, &b.touch), (2, &b.pan)]
            .into_iter()
            .filter(|(_, t)| t.fires(input, mods))
            .fold(None, |best: Option<(usize, usize)>, (i, t)| match best {
                Some((_, w)) if w >= t.weight() => best,
                _ => Some((i, t.weight())),
            })
            .map(|(i, _)| i);
        match picked {
            Some(0) => {
                let r = self.cfg.pinch_start_radius;
                let g = Gesture::Pinch { center: p, radius: r, anchor: Some((p, r)), last_wheel_ms: now_ms };
                self.begin_gesture(g, Some(button), now_ms, sink);
            }
            Some(1) => self.begin_gesture(Gesture::Touch, Some(button), now_ms, sink),
            Some(_) => self.begin_gesture(Gesture::Pan, Some(button), now_ms, sink),
            None => {}
        }
    }

    fn move_to(&mut self, p: Pt, now_ms: i64, sink: &mut dyn EventSink) {
        if p == self.pos {
            return;
        }
        self.pos = p;
        match self.gesture.clone() {
            Gesture::None => {}
            Gesture::Touch | Gesture::Pan => self.send_move(now_ms, sink),
            Gesture::Pinch { anchor: None, .. } => {} // the wheel owns the radius
            Gesture::Pinch { center, anchor: Some((a, r0)), last_wheel_ms, .. } => {
                let radius =
                    (r0 + (p.0 - a.0) - (p.1 - a.1)).clamp(self.cfg.pinch_min_radius, self.cfg.pinch_max_radius);
                self.gesture = Gesture::Pinch { center, radius, anchor: Some((a, r0)), last_wheel_ms };
                self.send_move(now_ms, sink);
            }
        }
    }

    fn wheel(&mut self, dy: f32, p: Pt, mods: Mods, now_ms: i64, sink: &mut dyn EventSink) {
        if !self.cfg.bindings.zoom.fires(Input::Wheel, mods) {
            return;
        }
        if matches!(self.gesture, Gesture::None) {
            self.pos = p;
            let g =
                Gesture::Pinch { center: p, radius: self.cfg.pinch_start_radius, anchor: None, last_wheel_ms: now_ms };
            self.begin_gesture(g, None, now_ms, sink);
        }
        if let Gesture::Pinch { center, radius, anchor: None, .. } = self.gesture {
            let next = (radius + dy * self.cfg.pinch_step).clamp(self.cfg.pinch_min_radius, self.cfg.pinch_max_radius);
            self.gesture = Gesture::Pinch { center, radius: next, anchor: None, last_wheel_ms: now_ms };
            if next != radius {
                self.send_move(now_ms, sink);
            }
        }
        // Any other gesture owns the cursor: the wheel is ignored.
    }

    // ---------- keyboard and text ----------

    fn key(&mut self, key: HostKey, pressed: bool, repeat: bool, mods: Mods, now_ms: i64, sink: &mut dyn EventSink) {
        let b = self.cfg.bindings;
        if b.fullscreen.fires(Input::Key(key), mods) {
            if pressed && !repeat {
                sink.host_action(HostAction::ToggleFullscreen);
            }
            return;
        }
        // A letter would be typed into a focused text field instead.
        if b.hide_ui.fires(Input::Key(key), mods) && self.text.is_none() {
            if pressed && !repeat {
                sink.host_action(HostAction::ToggleGameButtons);
            }
            return;
        }
        // The back binding stands for the Android back key; the Escape key itself is only that when it is bound.
        let key = if b.back.fires(Input::Key(key), mods) {
            HostKey::Escape
        } else if key == HostKey::Escape {
            return;
        } else {
            key
        };
        if key == HostKey::Escape && repeat {
            return;
        }
        if self.text.is_some() && key != HostKey::Escape {
            if pressed {
                self.edit_key(key, repeat, sink);
            }
            return;
        }
        let Some(key_code) = keymap::key_code(key) else { return };
        let (unicode, meta) = (keymap::unicode(key, mods), keymap::meta_state(mods));
        let ev = if pressed {
            GuestEvent::KeyDown { key_code, unicode, meta, time_ms: now_ms }
        } else {
            GuestEvent::KeyUp { key_code, unicode, meta, time_ms: now_ms }
        };
        sink.deliver(&ev);
    }

    fn edit_key(&mut self, key: HostKey, repeat: bool, sink: &mut dyn EventSink) {
        let Some(tf) = self.text.as_mut() else { return };
        match key {
            HostKey::Backspace => {
                let r = tf.model.back_range();
                self.replace_range(r, sink)
            }
            HostKey::Delete => {
                let r = tf.model.forward_range();
                self.replace_range(r, sink)
            }
            HostKey::Left => tf.model.left(),
            HostKey::Right => tf.model.right(),
            HostKey::Home => tf.model.home(),
            HostKey::End => tf.model.end(),
            HostKey::Enter if !repeat => {
                sink.deliver(&GuestEvent::TextShouldReturn { field: tf.field });
            }
            _ => {}
        }
    }

    fn replace_range(&mut self, range: Option<(usize, usize)>, sink: &mut dyn EventSink) {
        if let Some((loc, len)) = range {
            self.change(loc, len, "", sink);
        }
    }

    fn insert_text(&mut self, s: &str, sink: &mut dyn EventSink) {
        if let Some(tf) = &self.text {
            let at = tf.model.caret();
            self.change(at, 0, s, sink);
        }
    }

    /// Ask the guest whether `[loc, loc+len)` may become `with`; apply it if so.
    fn change(&mut self, loc: usize, len: usize, with: &str, sink: &mut dyn EventSink) {
        let Some(tf) = self.text.as_mut() else { return };
        let ev = GuestEvent::TextShouldChange {
            field: tf.field,
            location: loc as u32,
            length: len as u32,
            replacement: with.to_string(),
        };
        if sink.deliver(&ev) {
            tf.model.replace(loc, len, with);
            sink.deliver(&GuestEvent::TextDidChange { field: tf.field });
        }
    }
}
