use crate::event::*;
use crate::keymap;
use crate::mapper::{Config, Geometry, InputMapper};
use crate::text::TextModel;

fn cfg() -> Config {
    // 1:1 scale keeps the arithmetic readable.
    Config { content: None, window: (800, 600), ..Config::default() }
}

fn run(m: &mut InputMapper, evs: Vec<(HostEvent, i64)>) -> Recorder {
    let mut r = Recorder::new();
    for (e, t) in evs {
        m.handle(e, t, &mut r);
    }
    r
}

fn down(b: MouseButton, x: f32, y: f32) -> HostEvent {
    HostEvent::MouseButton { button: b, pressed: true, x, y, mods: Mods::default() }
}
fn up(b: MouseButton, x: f32, y: f32) -> HostEvent {
    HostEvent::MouseButton { button: b, pressed: false, x, y, mods: Mods::default() }
}
fn mv(x: f32, y: f32) -> HostEvent {
    HostEvent::MouseMove { x, y }
}
fn key(k: HostKey, pressed: bool) -> HostEvent {
    HostEvent::Key { key: k, pressed, repeat: false, mods: Mods::default() }
}
fn begin(id: i32, x: f32, y: f32, t: i64) -> GuestEvent {
    GuestEvent::TouchBegin { id, x, y, time_ms: t }
}
fn end(id: i32, x: f32, y: f32, t: i64) -> GuestEvent {
    GuestEvent::TouchEnd { id, x, y, time_ms: t }
}
fn moved(ids: &[i32], xs: &[f32], ys: &[f32], t: i64) -> GuestEvent {
    GuestEvent::TouchMove { ids: ids.to_vec(), xs: xs.to_vec(), ys: ys.to_vec(), time_ms: t }
}

#[test]
fn left_button_is_a_single_touch() {
    let mut m = InputMapper::new(cfg());
    let r = run(
        &mut m,
        vec![
            (mv(5.0, 5.0), 1),
            (down(MouseButton::Left, 10.0, 20.0), 2),
            (mv(10.0, 20.0), 3), // no movement: nothing
            (mv(30.0, 40.0), 4),
            (up(MouseButton::Left, 31.0, 41.0), 5),
            (mv(50.0, 50.0), 6), // no gesture: nothing
        ],
    );
    assert_eq!(
        r.events,
        vec![
            begin(0, 10.0, 20.0, 2),
            moved(&[0], &[30.0], &[40.0], 4),
            moved(&[0], &[31.0], &[41.0], 5),
            end(0, 31.0, 41.0, 5),
        ]
    );
    assert!(!m.gesture_active());
}

#[test]
fn release_without_motion_sends_only_end() {
    let mut m = InputMapper::new(cfg());
    let r = run(&mut m, vec![(down(MouseButton::Left, 1.0, 2.0), 0), (up(MouseButton::Left, 1.0, 2.0), 9)]);
    assert_eq!(r.events, vec![begin(0, 1.0, 2.0, 0), end(0, 1.0, 2.0, 9)]);
}

#[test]
fn coordinates_scale_to_content_size() {
    let mut m = InputMapper::new(Config { content: Some((1024.0, 768.0)), window: (512, 384), ..Config::default() });
    assert_eq!(m.to_content(256.0, 192.0), (512.0, 384.0));
    let r = run(&mut m, vec![(down(MouseButton::Left, 256.0, 192.0), 0)]);
    assert_eq!(r.events, vec![begin(0, 512.0, 384.0, 0)]);
    // A zero-sized window falls back to pixels instead of dividing by zero.
    let mut z = InputMapper::new(Config { window: (0, 0), ..Config::default() });
    assert_eq!(z.to_content(3.0, 4.0), (3.0, 4.0));
    let resized = HostEvent::Resized { width: 2048, height: 1536, logical_width: 2048, logical_height: 1536 };
    z.handle(resized, 0, &mut Recorder::new());
    assert_eq!(z.to_content(2048.0, 1536.0), (1024.0, 768.0));
}

#[test]
fn content_is_letterboxed_into_the_window() {
    // A 1000x1000 logical window shows a 1024x768 content at scale 1000/1024, centred vertically.
    let mut m = InputMapper::new(Config { content: Some((1024.0, 768.0)), window: (1000, 1000), ..Config::default() });
    let s = 1000.0 / 1024.0;
    let top = (1000.0 - 768.0 * s) / 2.0;
    assert_eq!(m.to_content(0.0, top), (0.0, 0.0));
    assert_eq!(m.to_content(1000.0, 1000.0 - top), (1024.0, 768.0));
    // Above the viewport: negative content y.
    assert!(m.to_content(500.0, 0.0).1 < 0.0);
    let g = m.geometry();
    let (wx, wy) = g.to_window(512.0, 384.0);
    assert!((wx - 500.0).abs() < 1e-3 && (wy - 500.0).abs() < 1e-3);
    // The game picks another content size; a high-DPI window reports logical and drawable sizes apart.
    m.set_content(1334.0, 750.0);
    m.handle(
        HostEvent::Resized { width: 2668, height: 1500, logical_width: 1334, logical_height: 750 },
        0,
        &mut Recorder::new(),
    );
    assert_eq!(m.to_content(667.0, 375.0), (667.0, 375.0));
    assert_eq!(Geometry { content: None, window: (5, 5) }.to_window(1.0, 2.0), (1.0, 2.0));
}

#[test]
fn right_drag_pans_with_two_fingers() {
    let mut m = InputMapper::new(cfg()); // spacing 80
    let r = run(
        &mut m,
        vec![
            (down(MouseButton::Right, 100.0, 100.0), 1),
            (mv(110.0, 90.0), 2),
            (up(MouseButton::Right, 110.0, 90.0), 3),
        ],
    );
    assert_eq!(
        r.events,
        vec![
            begin(0, 60.0, 100.0, 1),
            begin(1, 140.0, 100.0, 1),
            moved(&[0, 1], &[70.0, 150.0], &[90.0, 90.0], 2),
            end(0, 70.0, 90.0, 3),
            end(1, 150.0, 90.0, 3),
        ]
    );
}

#[test]
fn second_button_during_a_gesture_is_ignored() {
    let mut m = InputMapper::new(cfg());
    let r = run(
        &mut m,
        vec![
            (down(MouseButton::Left, 1.0, 1.0), 0),
            (down(MouseButton::Right, 1.0, 1.0), 1),
            (up(MouseButton::Right, 1.0, 1.0), 2),
            (down(MouseButton::Middle, 1.0, 1.0), 3),
            (up(MouseButton::Left, 1.0, 1.0), 4),
            (down(MouseButton::Middle, 1.0, 1.0), 5),
            (up(MouseButton::Middle, 1.0, 1.0), 6),
        ],
    );
    assert_eq!(r.events, vec![begin(0, 1.0, 1.0, 0), end(0, 1.0, 1.0, 4)]);
}

#[test]
fn ctrl_drag_pinches_around_the_press_point() {
    let mut m = InputMapper::new(cfg()); // start radius 100, min 20, max 400
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    let press = HostEvent::MouseButton { button: MouseButton::Left, pressed: true, x: 200.0, y: 200.0, mods: ctrl };
    let r = run(
        &mut m,
        vec![
            (press, 0),
            (mv(250.0, 200.0), 1), // right 50: radius 150
            (mv(250.0, 250.0), 2), // down 50: radius 100
            (mv(0.0, 600.0), 3),   // far left/down: clamped to 20
            (mv(2000.0, 0.0), 4),  // far right/up: clamped to 400
            (up(MouseButton::Left, 2000.0, 0.0), 5),
        ],
    );
    assert_eq!(
        r.events,
        vec![
            begin(0, 100.0, 200.0, 0),
            begin(1, 300.0, 200.0, 0),
            moved(&[0, 1], &[50.0, 350.0], &[200.0, 200.0], 1),
            moved(&[0, 1], &[100.0, 300.0], &[200.0, 200.0], 2),
            moved(&[0, 1], &[180.0, 220.0], &[200.0, 200.0], 3),
            moved(&[0, 1], &[-200.0, 600.0], &[200.0, 200.0], 4),
            end(0, -200.0, 200.0, 5),
            end(1, 600.0, 200.0, 5),
        ]
    );
}

#[test]
fn wheel_pinches_and_ends_on_idle() {
    let mut m = InputMapper::new(cfg()); // step 20, idle 150
    let mut r = Recorder::new();
    m.handle(HostEvent::Wheel { dy: 1.0, x: 300.0, y: 200.0, mods: Mods::default() }, 0, &mut r);
    m.handle(HostEvent::Wheel { dy: -2.0, x: 300.0, y: 200.0, mods: Mods::default() }, 50, &mut r);
    m.handle(mv(310.0, 210.0), 60, &mut r); // cursor motion does not move a wheel pinch
    m.tick(100, &mut r); // not idle yet
    assert!(m.gesture_active());
    m.tick(250, &mut r); // 200 ms since the last wheel event
    assert!(!m.gesture_active());
    m.tick(300, &mut r); // nothing to end
    assert_eq!(
        r.events,
        vec![
            begin(0, 200.0, 200.0, 0),
            begin(1, 400.0, 200.0, 0),
            moved(&[0, 1], &[180.0, 420.0], &[200.0, 200.0], 0),
            moved(&[0, 1], &[220.0, 380.0], &[200.0, 200.0], 50),
            end(0, 220.0, 200.0, 250),
            end(1, 380.0, 200.0, 250),
        ]
    );
}

#[test]
fn wheel_radius_is_clamped_and_unchanged_radius_sends_nothing() {
    let mut m = InputMapper::new(cfg());
    let mut r = Recorder::new();
    m.handle(HostEvent::Wheel { dy: 100.0, x: 0.0, y: 0.0, mods: Mods::default() }, 0, &mut r); // to max 400
    m.handle(HostEvent::Wheel { dy: 100.0, x: 0.0, y: 0.0, mods: Mods::default() }, 1, &mut r); // already at max: no move
    m.handle(HostEvent::Wheel { dy: -100.0, x: 0.0, y: 0.0, mods: Mods::default() }, 2, &mut r); // to min 20
    let moves: Vec<_> = r.events.iter().filter(|e| matches!(e, GuestEvent::TouchMove { .. })).collect();
    assert_eq!(moves.len(), 2);
    assert_eq!(*moves[0], moved(&[0, 1], &[-400.0, 400.0], &[0.0, 0.0], 0));
    assert_eq!(*moves[1], moved(&[0, 1], &[-20.0, 20.0], &[0.0, 0.0], 2));
}

#[test]
fn wheel_is_ignored_during_other_gestures_and_click_ends_a_wheel_pinch() {
    let mut m = InputMapper::new(cfg());
    let r = run(
        &mut m,
        vec![
            (down(MouseButton::Left, 1.0, 1.0), 0),
            (HostEvent::Wheel { dy: 1.0, x: 1.0, y: 1.0, mods: Mods::default() }, 1),
            (up(MouseButton::Left, 1.0, 1.0), 2),
            // wheel pinch interrupted by a click: pinch ends first, then the touch begins
            (HostEvent::Wheel { dy: 0.0, x: 50.0, y: 50.0, mods: Mods::default() }, 3),
            (down(MouseButton::Left, 60.0, 60.0), 4),
        ],
    );
    assert_eq!(
        r.events,
        vec![
            begin(0, 1.0, 1.0, 0),
            end(0, 1.0, 1.0, 2),
            begin(0, -50.0, 50.0, 3),
            begin(1, 150.0, 50.0, 3),
            end(0, -50.0, 50.0, 4),
            end(1, 150.0, 50.0, 4),
            begin(0, 60.0, 60.0, 4),
        ]
    );
}

#[test]
fn escape_is_back_and_other_keys_use_android_codes() {
    let mut m = InputMapper::new(cfg());
    let rep = HostEvent::Key { key: HostKey::Escape, pressed: true, repeat: true, mods: Mods::default() };
    let shift = Mods { shift: true, alt: true, ctrl: true };
    let r = run(
        &mut m,
        vec![
            (key(HostKey::Escape, true), 1),
            (rep, 2),
            (key(HostKey::Escape, false), 3),
            (HostEvent::Key { key: HostKey::Char('A'), pressed: true, repeat: false, mods: shift }, 4),
            (key(HostKey::Char('7'), true), 5),
            (key(HostKey::Space, false), 6),
            (key(HostKey::Other, true), 7),
            (key(HostKey::Char('!'), true), 8),
        ],
    );
    let k = |down: bool, key_code, unicode, meta, time_ms| {
        if down {
            GuestEvent::KeyDown { key_code, unicode, meta, time_ms }
        } else {
            GuestEvent::KeyUp { key_code, unicode, meta, time_ms }
        }
    };
    assert_eq!(
        r.events,
        vec![
            k(true, 4, 0, 0, 1),
            k(false, 4, 0, 0, 3),
            k(true, 29, 'A' as i32, 0x1003, 4),
            k(true, 14, '7' as i32, 0, 5),
            k(false, 62, ' ' as i32, 0, 6),
        ]
    );
}

#[test]
fn keymap_covers_every_key() {
    use HostKey::*;
    let codes: Vec<_> = [Enter, Backspace, Delete, Tab, Up, Down, Left, Right, Home, End, Char('z'), Char('0')]
        .into_iter()
        .map(keymap::key_code)
        .collect();
    assert_eq!(codes, [66, 67, 112, 61, 19, 20, 21, 22, 122, 123, 54, 7].map(Some));
    assert_eq!(keymap::key_code(F(11)), None);
    assert_eq!(keymap::key_code(Other), None);
    assert_eq!(keymap::unicode(Char('q'), Mods::default()), 'q' as i32);
    assert_eq!(keymap::unicode(Enter, Mods::default()), 0);
    assert_eq!(keymap::meta_state(Mods::default()), 0);
}

#[test]
fn f11_and_quit_are_host_actions() {
    let mut m = InputMapper::new(cfg());
    let rep = HostEvent::Key { key: HostKey::F(11), pressed: true, repeat: true, mods: Mods::default() };
    let r = run(
        &mut m,
        vec![(key(HostKey::F(11), true), 0), (rep, 1), (key(HostKey::F(11), false), 2), (HostEvent::Quit, 3)],
    );
    assert!(r.events.is_empty());
    assert_eq!(r.actions, [HostAction::ToggleFullscreen, HostAction::Quit]);
}

#[test]
fn window_events_map_to_lifecycle_and_surface() {
    let mut m = InputMapper::new(cfg());
    let r = run(
        &mut m,
        vec![
            (down(MouseButton::Right, 100.0, 100.0), 0),
            (HostEvent::FocusLost, 1),
            (HostEvent::Minimized, 2),
            (HostEvent::Restored, 3),
            (HostEvent::FocusGained, 4),
            (HostEvent::Resized { width: 640, height: 480, logical_width: 320, logical_height: 240 }, 5),
        ],
    );
    assert_eq!(
        r.events,
        vec![
            begin(0, 60.0, 100.0, 0),
            begin(1, 140.0, 100.0, 0),
            GuestEvent::TouchCancel { ids: vec![0, 1], xs: vec![60.0, 140.0], ys: vec![100.0, 100.0], time_ms: 1 },
            // focus loss and gain send no lifecycle; minimise and restore do
            GuestEvent::Lifecycle(Lifecycle::WillResignActive),
            GuestEvent::Lifecycle(Lifecycle::DidEnterBackground),
            GuestEvent::Lifecycle(Lifecycle::WillEnterForeground),
            GuestEvent::Lifecycle(Lifecycle::DidBecomeActive),
            GuestEvent::SurfaceChanged { width: 640, height: 480 },
        ]
    );
    assert!(!m.gesture_active());
}

#[test]
fn text_field_editing_flow() {
    for ev in [
        GuestEvent::TextShouldChange { field: 7, location: 0, length: 0, replacement: String::new() },
        GuestEvent::TextDidChange { field: 7 },
        GuestEvent::TextShouldReturn { field: 7 },
    ] {
        assert_eq!(ev.text_field(), Some(7));
    }
    assert_eq!(GuestEvent::WindowFocus(true).text_field(), None);
    let mut m = InputMapper::new(cfg());
    let mut r = Recorder::new();
    assert_eq!(m.text(), None);
    m.handle(HostEvent::TextInput("ignored".into()), 0, &mut r); // no focus
    m.handle(key(HostKey::Backspace, true), 0, &mut r); // no focus: plain key
    m.set_text_focus(Some((7, "ab")), &mut r);
    m.set_text_focus(Some((7, "ab")), &mut r); // refocus: no second start action
    assert_eq!(m.text().as_deref(), Some("ab"));
    r.events.clear();

    m.handle(HostEvent::TextInput("c".into()), 1, &mut r);
    m.handle(key(HostKey::Backspace, true), 1, &mut r);
    m.handle(key(HostKey::Backspace, false), 1, &mut r); // release ignored
    m.handle(key(HostKey::Home, true), 1, &mut r);
    m.handle(key(HostKey::Delete, true), 1, &mut r);
    m.handle(key(HostKey::Right, true), 1, &mut r);
    m.handle(key(HostKey::Left, true), 1, &mut r);
    m.handle(key(HostKey::End, true), 1, &mut r);
    m.handle(key(HostKey::Tab, true), 1, &mut r); // useless in a field
    m.handle(key(HostKey::Enter, true), 2, &mut r);
    let rep = HostEvent::Key { key: HostKey::Enter, pressed: true, repeat: true, mods: Mods::default() };
    m.handle(rep, 2, &mut r); // repeated Enter does not re-submit
    m.handle(key(HostKey::Escape, true), 3, &mut r); // Back still works

    let change = |location, length, replacement: &str| GuestEvent::TextShouldChange {
        field: 7,
        location,
        length,
        replacement: replacement.into(),
    };
    let did = GuestEvent::TextDidChange { field: 7 };
    assert_eq!(
        r.events,
        vec![
            change(2, 0, "c"),
            did.clone(),
            change(2, 1, ""),
            did.clone(),
            change(0, 1, ""),
            did,
            GuestEvent::TextShouldReturn { field: 7 },
            GuestEvent::KeyDown { key_code: 4, unicode: 0, meta: 0, time_ms: 3 },
        ]
    );
    assert_eq!(m.text().as_deref(), Some("b"));
    m.set_text_focus(None, &mut r);
    m.set_text_focus(None, &mut r);
    assert_eq!(r.actions, [HostAction::StartTextInput, HostAction::StopTextInput]);
    assert_eq!(m.text(), None);
}

#[test]
fn rejected_edits_leave_the_model_alone() {
    let mut m = InputMapper::new(cfg());
    let mut r = Recorder::new();
    m.set_text_focus(Some((1, "x")), &mut r);
    r.accept = false;
    m.handle(HostEvent::TextInput("y".into()), 0, &mut r);
    assert_eq!(m.text().as_deref(), Some("x"));
    assert_eq!(r.events.len(), 1, "no textDidChange after a refusal");
    // Backspace / Delete at the ends produce no event at all.
    r.accept = true;
    m.handle(key(HostKey::Delete, true), 0, &mut r);
    m.handle(key(HostKey::Home, true), 0, &mut r);
    m.handle(key(HostKey::Backspace, true), 0, &mut r);
    assert_eq!(r.events.len(), 1);
}

/// A guest field like the game's name fields: it upper-cases and filters each edit, sets the result itself and
/// answers NO; it takes only letters, digits and spaces.
struct GameField {
    text: String,
    calls: Vec<GuestEvent>,
}

impl EventSink for GameField {
    fn deliver(&mut self, ev: &GuestEvent) -> bool {
        self.calls.push(ev.clone());
        if let GuestEvent::TextShouldChange { location, length, replacement, .. } = ev {
            if replacement.chars().any(|c| !c.is_alphanumeric() && c != ' ') {
                return false;
            }
            let mut m = TextModel::new(&self.text);
            m.replace(*location as usize, *length as usize, &replacement.to_uppercase());
            self.text = m.text();
        }
        false
    }
    fn host_action(&mut self, _: HostAction) {}
    fn field_text(&mut self) -> Option<String> {
        Some(self.text.clone())
    }
}

#[test]
fn the_host_copy_follows_a_field_that_sets_its_own_text() {
    let mut m = InputMapper::new(cfg());
    let mut g = GameField { text: "MY WORLD".into(), calls: Vec::new() };
    m.set_text_focus(Some((3, "MY WORLD")), &mut g);
    // Typed and pasted text arrive one character at a time; refused ones and line breaks change nothing.
    m.handle(HostEvent::TextInput(" ab-ç\nğ".into()), 0, &mut g);
    assert_eq!(m.text().as_deref(), Some("MY WORLD ABÇĞ"));
    assert_eq!(m.text_caret(), Some(13));
    assert_eq!(g.calls.len(), 6, "one question per character, none for the line break");
    // The caret stays where the host put it, so Backspace removes the last character and Left/typing edit inside.
    m.handle(key(HostKey::Backspace, true), 0, &mut g);
    assert_eq!(g.text, "MY WORLD ABÇ");
    m.handle(key(HostKey::Left, true), 0, &mut g);
    m.handle(HostEvent::TextInput("ı".into()), 0, &mut g);
    assert_eq!(g.text, "MY WORLD ABIÇ");
    assert_eq!(m.text().as_deref(), Some("MY WORLD ABIÇ"));
    assert_eq!(m.text_caret(), Some(12));
    m.set_text_focus(None, &mut g);
    assert_eq!(m.text_caret(), None);
    m.handle(HostEvent::TextInput("x".into()), 0, &mut g);
    assert_eq!(g.text, "MY WORLD ABIÇ", "no field, no edit");
}

#[test]
fn keypad_enter_is_enter() {
    assert_eq!(keymap::host_key_from_hid(88), HostKey::Enter);
    assert_eq!(keymap::host_key_from_hid(40), HostKey::Enter);
}

#[test]
fn text_model_takes_over_the_guest_text() {
    let mut t = TextModel::new("abc");
    t.left();
    t.sync("abc");
    assert_eq!(t.caret(), 2, "the same text changes nothing");
    t.sync("aXbc");
    assert_eq!((t.text().as_str(), t.caret()), ("aXbc", 3));
    t.sync("");
    assert_eq!(t.caret(), 0, "clamped at the start");
    t.sync("x");
    t.home();
    t.sync("a\u{1F600}");
    assert_eq!(t.caret(), 3, "never between the halves of a pair");
}

#[test]
fn text_model_handles_surrogate_pairs() {
    let mut t = TextModel::new("a\u{1F600}b");
    assert_eq!(t.caret(), 4);
    t.left();
    assert_eq!(t.back_range(), Some((1, 2)));
    t.left();
    assert_eq!(t.caret(), 1);
    assert_eq!(t.forward_range(), Some((1, 2)));
    t.right();
    assert_eq!(t.caret(), 3);
    t.home();
    assert_eq!(t.back_range(), None);
    t.end();
    assert_eq!(t.forward_range(), None);
    t.replace(100, 5, "!"); // clamped to the end
    assert_eq!(t.text(), "a\u{1F600}b!");
    t.replace(1, 2, "");
    assert_eq!(t.text(), "ab!");
    assert_eq!(t.caret(), 1);
    // A lone high surrogate at the very end is a single unit.
    let lone = TextModel::new("");
    assert_eq!(lone.forward_range(), None);
    t.replace(0, 3, "");
    assert_eq!(t.text(), "");
    t.right();
    t.left();
    assert_eq!(t.caret(), 0);
}

#[test]
fn guest_events_know_their_java_targets() {
    let all = [
        (begin(0, 0.0, 0.0, 0), "nativeTouchesBegin", "(IFFJ)V"),
        (end(0, 0.0, 0.0, 0), "nativeTouchesEnd", "(IFFJ)V"),
        (moved(&[0], &[0.0], &[0.0], 0), "nativeTouchesMove", "([I[F[FJ)V"),
        (
            GuestEvent::TouchCancel { ids: vec![], xs: vec![], ys: vec![], time_ms: 0 },
            "nativeTouchesCancel",
            "([I[F[FJ)V",
        ),
        (GuestEvent::KeyDown { key_code: 4, unicode: 0, meta: 0, time_ms: 0 }, "nativeKeyDown", "(IIJI)Z"),
        (GuestEvent::KeyUp { key_code: 4, unicode: 0, meta: 0, time_ms: 0 }, "nativeKeyUp", "(IIJI)Z"),
        (GuestEvent::WindowFocus(true), "nativeOnWindowFocusChanged", "(Z)V"),
        (
            GuestEvent::TextShouldChange { field: 0, location: 0, length: 0, replacement: String::new() },
            "shouldChangeCharactersInRange",
            "(IIILjava/lang/String;)Z",
        ),
        (GuestEvent::TextDidChange { field: 0 }, "textDidChange", "(I)V"),
        (GuestEvent::TextShouldReturn { field: 0 }, "shouldReturn", "(I)Z"),
    ];
    for (ev, name, sig) in all {
        let t = ev.targets();
        assert_eq!((t.len(), t[0].method, t[0].signature), (1, name, sig));
    }
    for (l, name) in [
        (Lifecycle::WillResignActive, "applicationWillResignActive"),
        (Lifecycle::DidEnterBackground, "applicationDidEnterBackground"),
        (Lifecycle::WillEnterForeground, "applicationWillEnterForeground"),
        (Lifecycle::DidBecomeActive, "applicationDidBecomeActive"),
    ] {
        let t = GuestEvent::Lifecycle(l).targets();
        assert_eq!((t[0].class, t[0].method, t[0].signature), ("com/apportable/Lifecycle", name, "()V"));
    }
    let s = GuestEvent::SurfaceChanged { width: 1, height: 2 }.targets();
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].method, "nativeRootGeometryChanged");
}

#[test]
fn touch_positions_can_be_mapped_and_other_events_pass_through() {
    let f = |x: f32, y: f32| (x * 2.0, y + 1.0);
    assert_eq!(begin(0, 1.0, 2.0, 7).map_points(&f), begin(0, 2.0, 3.0, 7));
    assert_eq!(end(1, 1.0, 2.0, 7).map_points(&f), end(1, 2.0, 3.0, 7));
    assert_eq!(moved(&[0, 1], &[1.0, 2.0], &[3.0, 4.0], 5).map_points(&f), moved(&[0, 1], &[2.0, 4.0], &[4.0, 5.0], 5));
    let cancel = GuestEvent::TouchCancel { ids: vec![0], xs: vec![1.0], ys: vec![1.0], time_ms: 3 };
    assert_eq!(
        cancel.map_points(&f),
        GuestEvent::TouchCancel { ids: vec![0], xs: vec![2.0], ys: vec![2.0], time_ms: 3 }
    );
    assert_eq!(GuestEvent::WindowFocus(true).map_points(&f), GuestEvent::WindowFocus(true));
}

fn bound(b: Bindings) -> InputMapper {
    InputMapper::new(Config { bindings: b, ..cfg() })
}

fn with(ctrl: bool, shift: bool, alt: bool) -> Mods {
    Mods { ctrl, shift, alt }
}

fn click(b: MouseButton, mods: Mods) -> HostEvent {
    HostEvent::MouseButton { button: b, pressed: true, x: 10.0, y: 10.0, mods }
}

#[test]
fn trigger_needs_its_modifiers_and_counts_them() {
    let t = Trigger { mods: with(true, false, true), input: Input::Wheel };
    assert_eq!(t.weight(), 2);
    assert_eq!(Trigger::plain(Input::Wheel).weight(), 0);
    assert_eq!(Trigger { mods: with(false, true, false), input: Input::Wheel }.weight(), 1);
    assert!(t.fires(Input::Wheel, with(true, true, true)));
    assert!(!t.fires(Input::Wheel, with(true, false, false)));
    assert!(!t.fires(Input::Wheel, with(false, false, true)));
    assert!(!t.fires(Input::Mouse(MouseButton::Left), with(true, false, true)));
    let s = Trigger { mods: with(false, true, false), input: Input::Wheel };
    assert!(!s.fires(Input::Wheel, Mods::default()));
    assert!(s.fires(Input::Wheel, with(false, true, false)));
}

#[test]
fn remapped_mouse_buttons_pick_the_most_specific_gesture() {
    // Touch on the right button, pan on the middle one with shift, pinch on shift + right.
    let b = Bindings {
        touch: Trigger::plain(Input::Mouse(MouseButton::Right)),
        pan: Trigger { mods: with(false, true, false), input: Input::Mouse(MouseButton::Middle) },
        pinch: Trigger { mods: with(false, true, false), input: Input::Mouse(MouseButton::Right) },
        ..Bindings::default()
    };
    let first = |ev: HostEvent| {
        let mut m = bound(b);
        run(&mut m, vec![(ev, 0)]).events.len()
    };
    assert_eq!(first(click(MouseButton::Left, Mods::default())), 0, "nothing is bound to the left button");
    assert_eq!(first(click(MouseButton::Right, Mods::default())), 1, "touch");
    assert_eq!(first(click(MouseButton::Right, with(false, true, false))), 2, "pinch beats touch with shift");
    assert_eq!(first(click(MouseButton::Middle, with(false, true, false))), 2, "pan");
    assert_eq!(first(click(MouseButton::Middle, Mods::default())), 0, "pan needs shift");
}

#[test]
fn zoom_binding_decides_about_the_wheel() {
    let b = Bindings { zoom: Trigger { mods: with(true, false, false), input: Input::Wheel }, ..Bindings::default() };
    let wheel = |mods| HostEvent::Wheel { dy: 1.0, x: 5.0, y: 5.0, mods };
    let mut m = bound(b);
    assert!(run(&mut m, vec![(wheel(Mods::default()), 0)]).events.is_empty());
    assert!(!run(&mut m, vec![(wheel(with(true, false, false)), 1)]).events.is_empty());
}

#[test]
fn fullscreen_and_back_follow_their_bindings() {
    let b = Bindings {
        fullscreen: Trigger { mods: with(false, false, true), input: Input::Key(HostKey::Char('f')) },
        back: Trigger::plain(Input::Key(HostKey::Char('b'))),
        ..Bindings::default()
    };
    let alt_f =
        |pressed, repeat| HostEvent::Key { key: HostKey::Char('f'), pressed, repeat, mods: with(false, false, true) };
    let mut m = bound(b);
    let r = run(&mut m, vec![(alt_f(true, false), 0), (alt_f(true, true), 1), (alt_f(false, false), 2)]);
    assert_eq!(r.actions, vec![HostAction::ToggleFullscreen]);
    assert!(r.events.is_empty());
    // F11 is an ordinary (unmapped) key now, plain F is not Alt+F, Escape is no longer Back.
    let r = run(&mut m, vec![(key(HostKey::F(11), true), 3), (key(HostKey::Char('f'), true), 4)]);
    assert!(r.actions.is_empty());
    assert_eq!(r.events.len(), 1, "only the letter reaches the game");
    assert!(run(&mut m, vec![(key(HostKey::Escape, true), 5)]).events.is_empty());
    let r = run(&mut m, vec![(key(HostKey::Char('b'), true), 6), (key(HostKey::Char('b'), false), 7)]);
    assert_eq!(
        r.events,
        vec![
            GuestEvent::KeyDown { key_code: KEYCODE_BACK, unicode: 0, meta: 0, time_ms: 6 },
            GuestEvent::KeyUp { key_code: KEYCODE_BACK, unicode: 0, meta: 0, time_ms: 7 }
        ]
    );
}

fn wheel(dy: f32, x: f32, y: f32) -> HostEvent {
    HostEvent::Wheel { dy, x, y, mods: Mods::default() }
}

#[test]
fn wheel_in_a_world_zooms_one_step_per_notch_from_the_first() {
    let mut m = InputMapper::new(cfg());
    m.set_world_zoom(true);
    let r = run(
        &mut m,
        vec![
            (wheel(1.0, 300.4, 200.6), 0),     // the first notch zooms at once
            (wheel(-1.0, 300.0, 200.0), 1000), // also after a pause, in the other direction
            (wheel(3.0, 10.0, 20.0), 1001),    // a fast flick: three steps
            (wheel(f32::NAN, 0.0, 0.0), 1002), // nonsense is ignored
        ],
    );
    assert!(r.events.is_empty(), "no synthesized touches");
    assert!(!m.gesture_active());
    assert_eq!(
        r.actions,
        vec![
            HostAction::WheelZoom { steps: 1, at: (300, 201) },
            HostAction::WheelZoom { steps: -1, at: (300, 200) },
            HostAction::WheelZoom { steps: 3, at: (10, 20) },
        ]
    );
}

#[test]
fn wheel_fractions_add_up_and_a_reversal_starts_afresh() {
    let mut m = InputMapper::new(cfg());
    m.set_world_zoom(true);
    let r = run(
        &mut m,
        vec![
            (wheel(0.4, 1.0, 1.0), 0),
            (wheel(0.4, 1.0, 1.0), 1),
            (wheel(0.4, 1.0, 1.0), 2),   // 1.2: one step, 0.2 left
            (wheel(-0.5, 1.0, 1.0), 3),  // reversal drops the 0.2
            (wheel(-0.6, 1.0, 1.0), 4),  // -1.1: one step out
            (wheel(250.0, 1.0, 1.0), 5), // clamped
        ],
    );
    assert_eq!(
        r.actions,
        vec![
            HostAction::WheelZoom { steps: 1, at: (1, 1) },
            HostAction::WheelZoom { steps: -1, at: (1, 1) },
            HostAction::WheelZoom { steps: 100, at: (1, 1) },
        ]
    );
    // Leaving the world drops the rest and brings the pinch back.
    let mut r = Recorder::new();
    m.handle(wheel(0.5, 1.0, 1.0), 6, &mut r);
    m.set_world_zoom(false);
    m.set_world_zoom(true);
    m.handle(wheel(0.5, 1.0, 1.0), 7, &mut r);
    assert!(r.actions.is_empty());
    m.set_world_zoom(false);
    m.handle(wheel(1.0, 1.0, 1.0), 8, &mut r);
    assert!(r.actions.is_empty());
    assert!(m.gesture_active(), "outside a world the wheel pinches");
}

#[test]
fn wheel_zoom_ends_an_old_wheel_pinch_and_waits_for_other_gestures_and_the_binding() {
    let mut m = InputMapper::new(cfg());
    let mut r = Recorder::new();
    m.handle(wheel(1.0, 100.0, 100.0), 0, &mut r); // menu: a wheel pinch begins
    m.set_world_zoom(true); // the world shows
    m.handle(wheel(1.0, 100.0, 100.0), 1, &mut r);
    assert!(!m.gesture_active(), "the pinch ended");
    assert_eq!(r.actions, vec![HostAction::WheelZoom { steps: 1, at: (100, 100) }]);
    assert!(matches!(r.events.last(), Some(GuestEvent::TouchEnd { id: 1, .. })));
    // A held button owns the cursor.
    let mut r = run(&mut m, vec![(down(MouseButton::Left, 5.0, 5.0), 2), (wheel(1.0, 5.0, 5.0), 3)]);
    assert!(r.actions.is_empty());
    m.handle(up(MouseButton::Left, 5.0, 5.0), 4, &mut r);
    // A zoom binding with Ctrl leaves the plain wheel alone.
    let b = Bindings {
        zoom: Trigger { mods: Mods { ctrl: true, ..Mods::default() }, input: Input::Wheel },
        ..Bindings::default()
    };
    let mut m = InputMapper::new(Config { bindings: b, ..cfg() });
    m.set_world_zoom(true);
    let mut r = run(&mut m, vec![(wheel(1.0, 0.0, 0.0), 0)]);
    assert!(r.actions.is_empty());
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    m.handle(HostEvent::Wheel { dy: 1.0, x: 0.0, y: 0.0, mods: ctrl }, 1, &mut r);
    assert_eq!(r.actions, vec![HostAction::WheelZoom { steps: 1, at: (0, 0) }]);
}

#[test]
fn every_notch_counts_in_either_direction_whatever_the_wheel_reports() {
    let z = |steps| HostAction::WheelZoom { steps, at: (1, 1) };
    // GTK on Wayland: 1.5 per notch. Out, out, in, out, in in quick succession: one step each, no rest piling up.
    let mut m = InputMapper::new(cfg());
    m.set_world_zoom(true);
    let r = run(&mut m, [-1.5, -1.5, 1.5, -1.5, 1.5, 1.5].iter().map(|d| (wheel(*d, 1.0, 1.0), 0)).collect());
    assert_eq!(r.actions, vec![z(-1), z(-1), z(1), z(-1), z(1), z(1)]);
    // A high-resolution wheel in thirds: three thirds are a notch, also right after a reversal.
    let t = 1.0f32 / 3.0;
    let r = run(&mut m, [-t, -t, -t, t, t, t, -t, -t, -t].iter().map(|d| (wheel(*d, 1.0, 1.0), 0)).collect());
    assert_eq!(r.actions, vec![z(-1), z(1), z(-1)]);
    // A plain notch after a partial one the other way.
    let r = run(&mut m, vec![(wheel(-0.6, 1.0, 1.0), 0), (wheel(1.0, 1.0, 1.0), 1), (wheel(-1.0, 1.0, 1.0), 2)]);
    assert_eq!(r.actions, vec![z(1), z(-1)]);
}
