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
    m.handle(HostEvent::Wheel { dy: 1.0, x: 300.0, y: 200.0 }, 0, &mut r);
    m.handle(HostEvent::Wheel { dy: -2.0, x: 300.0, y: 200.0 }, 50, &mut r);
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
    m.handle(HostEvent::Wheel { dy: 100.0, x: 0.0, y: 0.0 }, 0, &mut r); // to max 400
    m.handle(HostEvent::Wheel { dy: 100.0, x: 0.0, y: 0.0 }, 1, &mut r); // already at max: no move
    m.handle(HostEvent::Wheel { dy: -100.0, x: 0.0, y: 0.0 }, 2, &mut r); // to min 20
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
            (HostEvent::Wheel { dy: 1.0, x: 1.0, y: 1.0 }, 1),
            (up(MouseButton::Left, 1.0, 1.0), 2),
            // wheel pinch interrupted by a click: pinch ends first, then the touch begins
            (HostEvent::Wheel { dy: 0.0, x: 50.0, y: 50.0 }, 3),
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
    assert_eq!(keymap::key_code(F11), None);
    assert_eq!(keymap::key_code(Other), None);
    assert_eq!(keymap::unicode(Char('q'), Mods::default()), 'q' as i32);
    assert_eq!(keymap::unicode(Enter, Mods::default()), 0);
    assert_eq!(keymap::meta_state(Mods::default()), 0);
}

#[test]
fn f11_and_quit_are_host_actions() {
    let mut m = InputMapper::new(cfg());
    let rep = HostEvent::Key { key: HostKey::F11, pressed: true, repeat: true, mods: Mods::default() };
    let r =
        run(&mut m, vec![(key(HostKey::F11, true), 0), (rep, 1), (key(HostKey::F11, false), 2), (HostEvent::Quit, 3)]);
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
