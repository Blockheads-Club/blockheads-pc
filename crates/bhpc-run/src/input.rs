//! Host input to guest Java natives: SDL events (`bhpc_app::Event`) -> `bhpc_input::HostEvent` -> the
//! `Window` / `Lifecycle` / `TextField` natives of the game, called through the fake JVM.

use crate::mainq::MainQueue;
use bhpc_app::{Event, Key, MouseButton as AppButton};
use bhpc_cpu::Cpu;
use bhpc_input::pad::axis_value;
use bhpc_input::{
    EventSink, Geometry, GuestEvent, HostAction, HostEvent, HostKey, Mods, MouseButton, PadAxis, PadButton, PadEvent,
};
use bhpc_jni::{Data, Obj, TouchPhase, Ty, Value, Vm};
use std::sync::Arc;

/// Translate a window-system event. Touch events are not forwarded (the mapper emulates touches from the mouse).
pub fn host_event(ev: &Event) -> Option<HostEvent> {
    let mods = |m: &bhpc_app::Mods| Mods { ctrl: m.ctrl, shift: m.shift, alt: m.alt };
    Some(match ev {
        Event::Quit => HostEvent::Quit,
        Event::Resized(m) => HostEvent::Resized {
            width: m.drawable.0,
            height: m.drawable.1,
            logical_width: m.logical.0,
            logical_height: m.logical.1,
        },
        Event::Focus(true) => HostEvent::FocusGained,
        Event::Focus(false) => HostEvent::FocusLost,
        Event::Visible(false) => HostEvent::Minimized,
        Event::Visible(true) => HostEvent::Restored,
        Event::Fullscreen(_) | Event::Touch { .. } => return None,
        Event::MouseMove { x, y } => HostEvent::MouseMove { x: *x, y: *y },
        Event::MouseButton { button, pressed, x, y } => {
            let button = match button {
                AppButton::Left => MouseButton::Left,
                AppButton::Right => MouseButton::Right,
                AppButton::Middle => MouseButton::Middle,
                AppButton::Other(_) => return None,
            };
            HostEvent::MouseButton { button, pressed: *pressed, x: *x, y: *y, mods: Mods::default() }
        }
        Event::Wheel { dy, .. } => HostEvent::Wheel { dy: *dy, x: 0.0, y: 0.0, mods: Mods::default() },
        Event::Key { key, pressed, repeat, mods: m } => {
            HostEvent::Key { key: host_key(*key), pressed: *pressed, repeat: *repeat, mods: mods(m) }
        }
        Event::Text(t) => HostEvent::TextInput(t.clone()),
        Event::PadAdded(id) => HostEvent::Pad(PadEvent::Added(*id)),
        Event::PadRemoved(id) => HostEvent::Pad(PadEvent::Removed(*id)),
        Event::PadButton { id, button, pressed } => {
            HostEvent::Pad(PadEvent::Button { pad: *id, button: PadButton::from_sdl(*button), pressed: *pressed })
        }
        Event::PadAxis { id, axis, value } => {
            HostEvent::Pad(PadEvent::Axis { pad: *id, axis: PadAxis::from_sdl(*axis), value: axis_value(*value) })
        }
    })
}

/// Drop every mouse move that is directly followed by another one: only the newest position of a burst matters.
/// The game throttles `nativeTouchesMove` to 60 Hz of wall-clock time (it ignores a move less than 1/60 s after the
/// last one it took), and the UI loop hands over everything that arrived during an iteration at once, so without this
/// the game would take the oldest position of each burst and be one step behind (the final one is lost until the
/// touch ends).
pub fn coalesce_moves(events: Vec<Event>) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::with_capacity(events.len());
    for ev in events {
        if matches!((&ev, out.last()), (Event::MouseMove { .. }, Some(Event::MouseMove { .. }))) {
            out.pop();
        }
        out.push(ev);
    }
    out
}

/// HID usage id (SDL scancode) to the keys the mapper knows.
pub fn host_key(k: Key) -> HostKey {
    bhpc_input::keymap::host_key_from_hid(k.0)
}

/// A position in content pixels (what touches carry) as a position in the screen pixels the Java views are laid out
/// in; `scale` is `Vm::content_scale`.
pub fn content_to_screen(scale: (f32, f32), x: f32, y: f32) -> (f32, f32) {
    (x * scale.0, y * scale.1)
}

/// Where a point of the Java view layer (screen pixels, `scale` as above) is on the window, in the unit of mouse
/// events: through the content surface and its letterboxed place in the window.
pub fn screen_to_window(geometry: &Geometry, scale: (f32, f32), x: f32, y: f32) -> (f32, f32) {
    geometry.to_window(x / scale.0, y / scale.1)
}

/// Delivers guest events by calling the game's natives through the fake JVM.
pub struct JniSink<'a> {
    pub vm: &'a Vm,
    pub cpu: &'a mut Cpu,
    /// Host-side requests collected while delivering (fullscreen, quit, text input).
    pub actions: Vec<HostAction>,
    /// Java exceptions and guest stops met while delivering.
    pub errors: Vec<String>,
    /// The guest was told the return key was pressed in the focused text field.
    pub returned: bool,
    /// Where touch, key and text natives run: the guest main thread (the only one with a GL context). `None`
    /// calls them on the current thread.
    pub main: Option<Arc<MainQueue>>,
    /// The game's controls, told when the controller's walking finger goes down or up (before the game sees it).
    pub game: Option<Arc<crate::gameui::GameUi>>,
}

/// Events whose natives the game handles on its main thread (the rest are lifecycle/surface/focus natives that
/// must not run there).
pub fn runs_on_main(ev: &GuestEvent) -> bool {
    !matches!(ev, GuestEvent::WindowFocus(_) | GuestEvent::SurfaceChanged { .. } | GuestEvent::Lifecycle(_))
}

/// Call the natives of `ev`; the result is the boolean answer (true unless a native answered 0) and the errors.
fn deliver_natives(vm: &Vm, cpu: &mut Cpu, ev: &GuestEvent, focus: Option<Obj>) -> (bool, Vec<String>) {
    let (mut result, mut errors) = (true, Vec::new());
    let text_target = ev.text_field().is_some();
    for (i, t) in ev.targets().iter().enumerate() {
        let args = args_for(vm, ev, i);
        let r = if t.method == "nativeRootGeometryChanged" {
            let a = vm.activity();
            vm.call_method(cpu, a, t.method, t.signature, &args)
        } else if let (true, Some(field)) = (text_target, focus) {
            // The TextField natives are instance methods.
            vm.call_method(cpu, field, t.method, t.signature, &args)
        } else {
            vm.call_static(cpu, t.class, t.method, t.signature, &args)
        };
        match r {
            Ok(v) => result = !matches!(v, Value::Int(0)),
            Err(e) => errors.push(format!("{}.{}: {e:?}", t.class, t.method)),
        }
    }
    (result, errors)
}

fn int_array(vm: &Vm, v: &[i32]) -> Value {
    let a = vm.new_prim_array(Ty::Int, v.len());
    vm.with_data(a, |d| *d = Data::Prim(Ty::Int, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    Value::Ref(Some(a))
}

fn float_array(vm: &Vm, v: &[f32]) -> Value {
    let a = vm.new_prim_array(Ty::Float, v.len());
    vm.with_data(a, |d| *d = Data::Prim(Ty::Float, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    Value::Ref(Some(a))
}

/// Java arguments of the `i`-th target of `ev` (see `GuestEvent::targets`).
pub fn args_for(vm: &Vm, ev: &GuestEvent, i: usize) -> Vec<Value> {
    match ev {
        GuestEvent::TouchBegin { id, x, y, time_ms } | GuestEvent::TouchEnd { id, x, y, time_ms } => {
            vec![Value::Int(*id), Value::Float(*x), Value::Float(*y), Value::Long(*time_ms)]
        }
        GuestEvent::TouchMove { ids, xs, ys, time_ms } | GuestEvent::TouchCancel { ids, xs, ys, time_ms } => {
            vec![int_array(vm, ids), float_array(vm, xs), float_array(vm, ys), Value::Long(*time_ms)]
        }
        GuestEvent::KeyDown { key_code, unicode, meta, time_ms }
        | GuestEvent::KeyUp { key_code, unicode, meta, time_ms } => {
            vec![Value::Int(*key_code), Value::Int(*unicode), Value::Long(*time_ms), Value::Int(*meta)]
        }
        GuestEvent::WindowFocus(f) => vec![Value::bool(*f)],
        GuestEvent::SurfaceChanged { width, height } if i == 0 => {
            vec![Value::Int(*width as i32), Value::Int(*height as i32)]
        }
        GuestEvent::SurfaceChanged { .. } | GuestEvent::Lifecycle(_) => vec![],
        GuestEvent::TextShouldChange { field, location, length, replacement } => vec![
            Value::Int(*field),
            Value::Int(*location as i32),
            Value::Int(*length as i32),
            Value::Ref(Some(vm.new_string(replacement))),
        ],
        GuestEvent::TextDidChange { field } | GuestEvent::TextShouldReturn { field } => vec![Value::Int(*field)],
    }
}

/// The touch of pointer 0 in `ev` for the control layer (`Vm::ui_touch`) when it is the only finger; multi-finger
/// gestures and the controller's walking finger never hit controls.
pub fn control_touch(ev: &GuestEvent) -> Option<(TouchPhase, f32, f32)> {
    match ev {
        GuestEvent::TouchBegin { id: 0, x, y, .. } => Some((TouchPhase::Begin, *x, *y)),
        GuestEvent::TouchEnd { id: 0, x, y, .. } => Some((TouchPhase::End, *x, *y)),
        GuestEvent::TouchMove { ids, xs, ys, .. } if ids[..] == [0] => Some((TouchPhase::Move, xs[0], ys[0])),
        GuestEvent::TouchCancel { ids, xs, ys, .. } if ids[..] == [0] => Some((TouchPhase::Cancel, xs[0], ys[0])),
        _ => None,
    }
}

impl EventSink for JniSink<'_> {
    fn deliver(&mut self, ev: &GuestEvent) -> bool {
        if let Some(g) = &self.game {
            g.track_walk(ev);
        }
        if let Some((phase, x, y)) = control_touch(ev) {
            // The touch is in content pixels, the Java views are laid out in screen pixels.
            let (x, y) = content_to_screen(self.vm.content_scale(), x, y);
            match self.vm.ui_touch(self.cpu, phase, x, y) {
                Ok(true) => return true,
                Ok(false) => {}
                Err(e) => self.errors.push(format!("ui touch: {e:?}")),
            }
        }
        // The game's own views take screen pixels, landscape like the window (the Java views, hit-tested by
        // `ui_touch`, are the ones laid out in a portrait space), so only the content-to-screen scale applies.
        let mapped = ev.map_points(&|x, y| content_to_screen(self.vm.content_scale(), x, y));
        let ev = &mapped;
        let text_target = ev.text_field().is_some();
        let focus = self.vm.text_focus();
        if text_target && focus.is_none() {
            return false;
        }
        let (result, errors) = match &self.main {
            Some(q) if runs_on_main(ev) => {
                let (vm, ev) = (self.vm.clone(), ev.clone());
                q.call(self.cpu, move |c| deliver_natives(&vm, c, &ev, focus))
            }
            _ => deliver_natives(self.vm, self.cpu, ev, focus),
        };
        self.errors.extend(errors);
        if let (GuestEvent::TextShouldChange { location, length, replacement, .. }, true, Some(field)) =
            (ev, result, focus)
        {
            self.vm.replace_text(field, *location as usize, *length as usize, replacement);
        }
        if matches!(ev, GuestEvent::TextShouldReturn { .. }) {
            self.returned = true;
        }
        result
    }

    fn host_action(&mut self, action: HostAction) {
        self.actions.push(action);
    }
}
