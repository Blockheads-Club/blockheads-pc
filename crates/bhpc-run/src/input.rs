//! Host input to guest Java natives: SDL events (`bhpc_app::Event`) -> `bhpc_input::HostEvent` -> the
//! `Window` / `Lifecycle` / `TextField` natives of the game, called through the fake JVM.

use bhpc_app::{Event, Key, MouseButton as AppButton};
use bhpc_cpu::Cpu;
use bhpc_input::{EventSink, GuestEvent, HostAction, HostEvent, HostKey, Mods, MouseButton};
use bhpc_jni::{Data, Ty, Value, Vm};

/// Translate a window-system event. Touch events are not forwarded (the mapper emulates touches from the mouse).
pub fn host_event(ev: &Event) -> Option<HostEvent> {
    let mods = |m: &bhpc_app::Mods| Mods { ctrl: m.ctrl, shift: m.shift, alt: m.alt };
    Some(match ev {
        Event::Quit => HostEvent::Quit,
        Event::Resized(m) => HostEvent::Resized { width: m.drawable.0, height: m.drawable.1 },
        Event::Focus(true) => HostEvent::FocusGained,
        Event::Focus(false) => HostEvent::FocusLost,
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
        Event::Wheel { dy, .. } => HostEvent::Wheel { dy: *dy, x: 0.0, y: 0.0 },
        Event::Key { key, pressed, repeat, mods: m } => {
            HostEvent::Key { key: host_key(*key), pressed: *pressed, repeat: *repeat, mods: mods(m) }
        }
        Event::Text(t) => HostEvent::TextInput(t.clone()),
    })
}

/// HID usage id (SDL scancode) to the keys the mapper knows.
pub fn host_key(k: Key) -> HostKey {
    match k.0 {
        n @ 4..=29 => HostKey::Char((b'a' + (n - 4) as u8) as char),
        n @ 30..=38 => HostKey::Char((b'1' + (n - 30) as u8) as char),
        39 => HostKey::Char('0'),
        40 => HostKey::Enter,
        41 => HostKey::Escape,
        42 => HostKey::Backspace,
        43 => HostKey::Tab,
        44 => HostKey::Space,
        68 => HostKey::F11,
        74 => HostKey::Home,
        76 => HostKey::Delete,
        77 => HostKey::End,
        79 => HostKey::Right,
        80 => HostKey::Left,
        81 => HostKey::Down,
        82 => HostKey::Up,
        _ => HostKey::Other,
    }
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

impl EventSink for JniSink<'_> {
    fn deliver(&mut self, ev: &GuestEvent) -> bool {
        let mut result = true;
        let text_target = ev.text_field().is_some();
        let focus = self.vm.text_focus();
        if text_target && focus.is_none() {
            return false;
        }
        for (i, t) in ev.targets().iter().enumerate() {
            let args = args_for(self.vm, ev, i);
            let r = if t.method == "nativeRootGeometryChanged" {
                let a = self.vm.activity();
                self.vm.call_method(self.cpu, a, t.method, t.signature, &args)
            } else if let (true, Some(field)) = (text_target, focus) {
                // The TextField natives are instance methods.
                self.vm.call_method(self.cpu, field, t.method, t.signature, &args)
            } else {
                self.vm.call_static(self.cpu, t.class, t.method, t.signature, &args)
            };
            match r {
                Ok(v) => result = !matches!(v, Value::Int(0)),
                Err(e) => self.errors.push(format!("{}.{}: {e:?}", t.class, t.method)),
            }
        }
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
