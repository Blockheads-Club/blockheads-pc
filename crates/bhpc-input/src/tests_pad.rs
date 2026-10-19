//! Controller mapping with fake controller events.

use crate::event::*;
use crate::grid::{Grid, Mat4};
use crate::mapper::{Config, InputMapper};
use crate::pad::*;

fn cfg() -> Config {
    Config { content: Some((1000.0, 800.0)), window: (1000, 800), ..Config::default() }
}

fn btn(pad: u32, button: PadButton, pressed: bool) -> HostEvent {
    HostEvent::Pad(PadEvent::Button { pad, button, pressed })
}
fn axis(pad: u32, axis: PadAxis, value: f32) -> HostEvent {
    HostEvent::Pad(PadEvent::Axis { pad, axis, value })
}
fn added(pad: u32) -> HostEvent {
    HostEvent::Pad(PadEvent::Added(pad))
}

fn mapper() -> (InputMapper, Recorder) {
    let mut m = InputMapper::new(cfg());
    let mut r = Recorder::new();
    m.handle(added(7), 0, &mut r);
    (m, r)
}

const ID: Mat4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// A camera 48.3 tiles away (tiles are 20 px) looking at tile (100.5, 50.5) in the middle of a 1000x800 picture, like the game's.
fn grid() -> Grid {
    let mut mv = ID;
    (mv[12], mv[13], mv[14]) = (-100.5, -50.5, -48.284_27);
    let f = 2.414_213_5; // cot(22.5 degrees)
    let mut pj: Mat4 = [0.0; 16];
    (pj[0], pj[5], pj[10], pj[11], pj[14]) = (f / 1.25, f, -1.002, -1.0, -2.002);
    Grid::from_gl(&mv, &pj, [0, 0, 1000, 800], (1000.0, 800.0)).unwrap()
}

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05
}

#[test]
fn sdl_numbers_and_values() {
    assert_eq!(PadButton::from_sdl(0), PadButton::A);
    assert_eq!(PadButton::from_sdl(6), PadButton::Start);
    assert_eq!(PadButton::from_sdl(14), PadButton::Right);
    assert_eq!(PadButton::from_sdl(15), PadButton::Other);
    assert_eq!(PadAxis::from_sdl(0), PadAxis::LeftX);
    assert_eq!(PadAxis::from_sdl(5), PadAxis::RightTrigger);
    assert_eq!(PadAxis::from_sdl(6), PadAxis::Other);
    assert_eq!(axis_value(32767), 1.0);
    assert_eq!(axis_value(-32768), -1.0);
    assert_eq!(axis_value(0), 0.0);
}

#[test]
fn a_taps_at_the_cursor_and_the_cursor_shows_when_the_pad_is_used() {
    let (mut m, mut r) = mapper();
    assert_eq!(m.pad().active(), Some(7));
    assert_eq!(m.pad().cursor(), None, "hidden until the controller is used");
    m.handle(btn(7, PadButton::A, true), 10, &mut r);
    assert_eq!(m.pad().cursor(), Some((500.0, 400.0)));
    assert!(m.pad().touching());
    m.handle(btn(7, PadButton::A, false), 90, &mut r);
    assert_eq!(
        r.events,
        vec![
            GuestEvent::TouchBegin { id: CURSOR_FINGER, x: 500.0, y: 400.0, time_ms: 10 },
            GuestEvent::TouchEnd { id: CURSOR_FINGER, x: 500.0, y: 400.0, time_ms: 90 },
        ]
    );
    // A release without a press does nothing; the mouse hides the cursor again.
    m.handle(btn(7, PadButton::A, false), 95, &mut r);
    assert_eq!(r.events.len(), 2);
    m.handle(HostEvent::MouseMove { x: 1.0, y: 1.0 }, 96, &mut r);
    assert_eq!(m.pad().cursor(), None);
}

#[test]
fn the_right_stick_moves_the_cursor_and_drags_while_a_is_held() {
    let (mut m, mut r) = mapper();
    m.tick(0, &mut r);
    m.handle(axis(7, PadAxis::RightX, 1.0), 0, &mut r);
    assert_eq!(m.pad().cursor(), Some((500.0, 400.0)), "shown, not moved yet");
    m.tick(20, &mut r); // 20 ms at full speed (700/s): 14 px
    let (x, y) = m.pad().cursor().unwrap();
    assert!((x - 514.0).abs() < 0.01 && y == 400.0, "{x}");
    // Long gaps count as 50 ms.
    m.tick(1000, &mut r);
    assert!((m.pad().cursor().unwrap().0 - 549.0).abs() < 0.01);
    m.handle(btn(7, PadButton::A, true), 1001, &mut r);
    m.tick(1021, &mut r);
    let moved = r.events.iter().filter(|e| matches!(e, GuestEvent::TouchMove { .. })).count();
    assert_eq!(moved, 1, "{:?}", r.events);
    // Inside the dead zone: no movement, and without a grid no snap.
    m.handle(axis(7, PadAxis::RightX, 0.1), 1022, &mut r);
    let at = m.pad().cursor();
    m.tick(1042, &mut r);
    assert_eq!(m.pad().cursor(), at);
    // The cursor stays inside the picture.
    m.handle(axis(7, PadAxis::RightY, -1.0), 1043, &mut r);
    for t in 0..200 {
        m.tick(1100 + t * 50, &mut r);
    }
    assert_eq!(m.pad().cursor().unwrap().1, 0.0);
    // Half tilt is slower than a quarter of full speed (quadratic response).
    let (mut m2, mut r2) = mapper();
    m2.handle(axis(7, PadAxis::RightX, 0.625), 0, &mut r2);
    m2.tick(0, &mut r2);
    m2.tick(40, &mut r2);
    let dx = m2.pad().cursor().unwrap().0 - 500.0;
    assert!(dx > 0.0 && dx < 700.0 * 0.04 * 0.3, "{dx}");
    // R3 brings it back to the middle.
    m2.handle(btn(7, PadButton::RightStick, true), 50, &mut r2);
    m2.handle(btn(7, PadButton::RightStick, false), 51, &mut r2);
    assert_eq!(m2.pad().cursor(), Some((500.0, 400.0)));
}

#[test]
fn the_dpad_steps_the_cursor_and_repeats() {
    let (mut m, mut r) = mapper();
    m.handle(btn(7, PadButton::Right, true), 0, &mut r);
    assert_eq!(m.pad().cursor(), Some((532.0, 400.0)));
    m.tick(100, &mut r);
    assert_eq!(m.pad().cursor(), Some((532.0, 400.0)), "no repeat before the delay");
    m.handle(btn(7, PadButton::Down, true), 150, &mut r); // diagonal
    assert_eq!(m.pad().cursor(), Some((564.0, 432.0)));
    m.tick(470, &mut r);
    assert_eq!(m.pad().cursor(), Some((596.0, 464.0)), "repeats after 320 ms");
    m.tick(500, &mut r);
    assert_eq!(m.pad().cursor(), Some((596.0, 464.0)));
    m.tick(560, &mut r);
    assert_eq!(m.pad().cursor(), Some((628.0, 496.0)), "then every 90 ms");
    m.handle(btn(7, PadButton::Right, false), 600, &mut r); // only down is left
    m.tick(650, &mut r);
    assert_eq!(m.pad().cursor(), Some((628.0, 528.0)));
    m.handle(btn(7, PadButton::Down, false), 700, &mut r);
    m.tick(2000, &mut r);
    assert_eq!(m.pad().cursor(), Some((628.0, 528.0)));
    m.handle(btn(7, PadButton::Up, true), 2001, &mut r);
    m.handle(btn(7, PadButton::Left, true), 2002, &mut r);
    assert_eq!(m.pad().cursor(), Some((596.0, 464.0)));
    assert!(r.events.is_empty(), "moving without A sends nothing");
}

#[test]
fn the_cursor_snaps_to_tiles() {
    let g = grid();
    // Tiles are 20 px: the middle of the picture is the centre of tile (100, 50).
    assert!(near(g.screen((100.5, 50.5)).unwrap(), (500.0, 400.0)));
    assert!((g.tile_size((500.0, 400.0)) - 20.0).abs() < 0.2, "{}", g.tile_size((500.0, 400.0)));
    let (mut m, mut r) = mapper();
    m.pad_mut().set_grid(Some(g));
    assert!((m.pad().tile_size().unwrap() - 20.0).abs() < 0.2);
    // D-pad: one tile, up the screen is up the world.
    m.handle(btn(7, PadButton::Up, true), 0, &mut r);
    assert!(near(m.pad().cursor().unwrap(), (500.0, 380.0)), "{:?}", m.pad().cursor());
    m.handle(btn(7, PadButton::Up, false), 1, &mut r);
    m.handle(btn(7, PadButton::Right, true), 2, &mut r);
    assert!(near(m.pad().cursor().unwrap(), (520.0, 380.0)));
    m.handle(btn(7, PadButton::Right, false), 3, &mut r);
    // The stick moves freely, then the release snaps to the tile under the cursor.
    m.handle(axis(7, PadAxis::RightX, 1.0), 4, &mut r);
    m.tick(10, &mut r);
    m.tick(20, &mut r); // 7 px
    assert!(near(m.pad().cursor().unwrap(), (527.0, 380.0)), "{:?}", m.pad().cursor());
    m.handle(axis(7, PadAxis::RightX, 0.0), 21, &mut r);
    m.tick(30, &mut r);
    assert!(near(m.pad().cursor().unwrap(), (520.0, 380.0)), "{:?}", m.pad().cursor());
    m.tick(40, &mut r); // only once
    assert!(near(m.pad().cursor().unwrap(), (520.0, 380.0)));
    let w = g.world((520.0, 380.0)).unwrap();
    assert!(near(w, (101.5, 51.5)), "{w:?}");
    // Unusable matrices.
    assert!(Grid::from_gl(&[0.0; 16], &[0.0; 16], [0, 0, 1000, 800], (1000.0, 800.0)).is_none());
    assert!(Grid::from_gl(&ID, &ID, [0, 0, 0, 800], (1000.0, 800.0)).is_none());
    assert!(Grid::from_gl(&ID, &ID, [0, 0, 10, 10], (0.0, 800.0)).is_none());
}

#[test]
fn grids_with_a_flipped_axis_or_a_horizon() {
    // An orthographic camera with a flipped y axis: world y grows down the screen.
    let mut mv = ID;
    (mv[0], mv[5]) = (0.1, -0.1);
    let g = Grid::from_gl(&mv, &ID, [0, 0, 1000, 800], (1000.0, 800.0)).unwrap();
    let p = g.step((500.0, 400.0), 0, 1);
    assert!(p.1 > 400.0, "down the screen: {p:?}");
    // A projection whose horizon (w = 0) crosses the picture: x = -1 maps nowhere.
    let mut tilt = ID;
    tilt[3] = 1.0; // w = x + 1
    let g = Grid::from_gl(&ID, &tilt, [0, 0, 1000, 800], (1000.0, 800.0)).unwrap();
    assert_eq!(g.screen((-1.0, 0.0)), None);
    // Off the plane entirely: nothing to map.
    assert_eq!(g.tile_size((f32::NAN, 0.0)), 0.0);
    assert!(g.step((f32::NAN, 0.0), 1, 0).0.is_nan());
    assert!(g.snap((f32::NAN, 0.0)).0.is_nan());
    // Singular matrices.
    let mut flat = ID;
    (flat[0], flat[5]) = (0.0, 0.0);
    assert!(Grid::from_gl(&flat, &ID, [0, 0, 1000, 800], (1000.0, 800.0)).is_none());
}

#[test]
fn the_left_stick_walks_with_a_finger_on_the_games_dpad() {
    let (mut m, mut r) = mapper();
    // No D-pad on screen: the stick does nothing.
    m.handle(axis(7, PadAxis::LeftX, 1.0), 0, &mut r);
    assert!(r.events.is_empty());
    m.handle(axis(7, PadAxis::LeftX, 0.0), 1, &mut r);
    m.pad_mut().set_dpad(Some((100.0, 650.0)));
    assert!(!m.pad().walking());
    m.handle(axis(7, PadAxis::LeftX, 1.0), 2, &mut r);
    assert!(m.pad().walking());
    m.handle(axis(7, PadAxis::LeftY, 0.0), 3, &mut r); // unchanged position: nothing
    m.handle(axis(7, PadAxis::LeftX, -0.5), 4, &mut r);
    m.handle(btn(7, PadButton::A, true), 5, &mut r); // a second finger at the cursor
    m.handle(btn(7, PadButton::Down, true), 6, &mut r); // the cursor moves: both fingers in the move
    m.handle(axis(7, PadAxis::LeftX, 0.1), 7, &mut r); // centred: the finger lifts
    m.handle(axis(7, PadAxis::LeftTrigger, 1.0), 8, &mut r); // triggers do nothing
    let reach = 60.0 * ((0.5 - 0.25) / 0.75);
    assert_eq!(
        r.events,
        vec![
            GuestEvent::TouchBegin { id: WALK_FINGER, x: 100.0, y: 650.0, time_ms: 2 },
            GuestEvent::TouchMove { ids: vec![WALK_FINGER], xs: vec![160.0], ys: vec![650.0], time_ms: 2 },
            GuestEvent::TouchMove { ids: vec![WALK_FINGER], xs: vec![100.0 - reach], ys: vec![650.0], time_ms: 4 },
            GuestEvent::TouchBegin { id: CURSOR_FINGER, x: 500.0, y: 400.0, time_ms: 5 },
            GuestEvent::TouchMove {
                ids: vec![CURSOR_FINGER, WALK_FINGER],
                xs: vec![500.0, 100.0 - reach],
                ys: vec![432.0, 650.0],
                time_ms: 6
            },
            GuestEvent::TouchEnd { id: WALK_FINGER, x: 100.0 - reach, y: 650.0, time_ms: 7 },
        ]
    );
    // A slight tilt still moves the finger a fifth of the way.
    m.handle(axis(7, PadAxis::LeftX, 0.0), 8, &mut Recorder::new());
    let mut r = Recorder::new();
    m.handle(axis(7, PadAxis::LeftY, 0.26), 9, &mut r);
    assert_eq!(
        r.events[1],
        GuestEvent::TouchMove { ids: vec![0, 2], xs: vec![500.0, 100.0], ys: vec![432.0, 662.0], time_ms: 9 }
    );
}

#[test]
fn buttons_for_back_zoom_pause_and_the_games_buttons() {
    let (mut m, mut r) = mapper();
    for b in [
        PadButton::B,
        PadButton::X,
        PadButton::Y,
        PadButton::LeftShoulder,
        PadButton::RightShoulder,
        PadButton::Start,
        PadButton::Back,
        PadButton::Guide,
        PadButton::LeftStick,
        PadButton::Other,
    ] {
        m.handle(btn(7, b, true), 1, &mut r);
        m.handle(btn(7, b, false), 2, &mut r);
    }
    assert_eq!(
        r.events,
        vec![
            GuestEvent::KeyDown { key_code: KEYCODE_BACK, unicode: 0, meta: 0, time_ms: 1 },
            GuestEvent::KeyUp { key_code: KEYCODE_BACK, unicode: 0, meta: 0, time_ms: 2 },
        ]
    );
    assert_eq!(
        r.actions,
        vec![
            HostAction::ToggleGameButtons,
            HostAction::ZoomCycle,
            HostAction::ZoomOut,
            HostAction::ZoomIn,
            HostAction::Pause,
        ]
    );
}

#[test]
fn the_first_controller_wins_until_it_is_unplugged() {
    let (mut m, mut r) = mapper();
    m.handle(added(9), 0, &mut r);
    m.handle(added(7), 0, &mut r); // a repeated announcement changes nothing
    m.handle(btn(9, PadButton::A, true), 1, &mut r);
    m.handle(axis(9, PadAxis::RightX, 1.0), 1, &mut r);
    assert!(r.events.is_empty() && m.pad().cursor().is_none(), "the second controller is ignored");
    m.handle(btn(7, PadButton::A, true), 2, &mut r);
    m.pad_mut().set_dpad(Some((10.0, 10.0)));
    m.handle(axis(7, PadAxis::LeftX, 1.0), 3, &mut r);
    // Unplugging the other one does nothing; unplugging the active one lifts its fingers.
    m.handle(HostEvent::Pad(PadEvent::Removed(9)), 4, &mut r);
    assert_eq!(m.pad().active(), Some(7));
    r.events.clear();
    m.handle(HostEvent::Pad(PadEvent::Removed(7)), 5, &mut r);
    assert_eq!(
        r.events,
        vec![GuestEvent::TouchCancel {
            ids: vec![CURSOR_FINGER, WALK_FINGER],
            xs: vec![500.0, 70.0],
            ys: vec![400.0, 10.0],
            time_ms: 5
        }]
    );
    assert_eq!(m.pad().active(), None);
    assert_eq!(m.pad().cursor(), None);
    // No controller: ticks do nothing.
    m.tick(100, &mut r);
    // The next one plugged in takes over.
    m.handle(added(9), 6, &mut r);
    m.handle(btn(9, PadButton::X, true), 7, &mut r);
    assert_eq!(r.actions, vec![HostAction::ToggleGameButtons]);
}

#[test]
fn focus_loss_and_minimising_lift_the_fingers() {
    for ev in [HostEvent::FocusLost, HostEvent::Minimized] {
        let (mut m, mut r) = mapper();
        m.handle(btn(7, PadButton::A, true), 1, &mut r);
        m.handle(btn(7, PadButton::Right, true), 1, &mut r);
        m.handle(axis(7, PadAxis::RightX, 1.0), 1, &mut r);
        r.events.clear();
        m.handle(ev, 2, &mut r);
        assert_eq!(
            r.events[0],
            GuestEvent::TouchCancel { ids: vec![CURSOR_FINGER], xs: vec![532.0], ys: vec![400.0], time_ms: 2 }
        );
        // Held directions and sticks are forgotten.
        m.tick(10, &mut r);
        m.tick(2000, &mut r);
        assert_eq!(m.pad().cursor(), Some((532.0, 400.0)));
        // Nothing held: nothing to cancel.
        let mut r = Recorder::new();
        m.pad_mut().release(3, &mut r);
        assert!(r.events.is_empty());
    }
    // Without the focus the controller is ignored until it comes back; plugging in still counts.
    let (mut m, mut r) = mapper();
    m.handle(HostEvent::FocusLost, 1, &mut r);
    m.handle(added(8), 2, &mut r);
    m.handle(btn(7, PadButton::X, true), 2, &mut r);
    m.handle(axis(7, PadAxis::RightX, 1.0), 2, &mut r);
    assert!(r.actions.is_empty() && m.pad().cursor().is_none());
    m.handle(HostEvent::FocusGained, 3, &mut r);
    m.handle(btn(7, PadButton::X, true), 4, &mut r);
    assert_eq!(r.actions, vec![HostAction::ToggleGameButtons]);
}

#[test]
fn the_mouse_and_the_controller_share_pointer_zero() {
    let (mut m, mut r) = mapper();
    let left =
        |pressed| HostEvent::MouseButton { button: MouseButton::Left, pressed, x: 5.0, y: 5.0, mods: Mods::default() };
    // A mouse touch in progress: A does nothing.
    m.handle(left(true), 1, &mut r);
    m.handle(btn(7, PadButton::A, true), 2, &mut r);
    assert!(!m.pad().touching());
    m.handle(btn(7, PadButton::A, false), 3, &mut r);
    m.handle(left(false), 4, &mut r);
    // A touch from the controller: the mouse button does nothing.
    m.handle(btn(7, PadButton::A, true), 5, &mut r);
    m.handle(left(true), 6, &mut r);
    assert!(!m.gesture_active());
    assert_eq!(r.events.iter().filter(|e| matches!(e, GuestEvent::TouchBegin { .. })).count(), 2);
}

#[test]
fn bounds_follow_the_content_and_controllers_can_be_off() {
    let (mut m, mut r) = mapper();
    m.handle(btn(7, PadButton::Right, true), 0, &mut r);
    m.set_content(400.0, 300.0);
    assert_eq!(m.pad().cursor(), Some((400.0, 300.0)), "clamped into the smaller picture");
    m.set_content(400.0, 300.0);
    let mut off = InputMapper::new(Config { pad: PadConfig { enabled: false, ..PadConfig::default() }, ..cfg() });
    let mut r = Recorder::new();
    off.handle(added(1), 0, &mut r);
    off.handle(btn(1, PadButton::X, true), 0, &mut r);
    off.tick(5, &mut r);
    assert_eq!(off.pad().active(), None);
    assert!(r.actions.is_empty());
    // Without a content size the window size bounds the cursor.
    let mut m = InputMapper::new(Config { content: None, window: (640, 480), ..Config::default() });
    assert_eq!(m.pad().cursor(), None);
    let mut r = Recorder::new();
    m.handle(added(1), 0, &mut r);
    m.handle(btn(1, PadButton::B, true), 0, &mut r);
    assert_eq!(m.pad().cursor(), Some((320.0, 240.0)));
}

#[test]
fn the_hide_buttons_key_is_a_host_action_but_not_while_typing() {
    let mut m = InputMapper::new(cfg());
    let mut r = Recorder::new();
    let h = |pressed, repeat| HostEvent::Key { key: HostKey::Char('h'), pressed, repeat, mods: Mods::default() };
    m.handle(h(true, false), 0, &mut r);
    m.handle(h(true, true), 1, &mut r);
    m.handle(h(false, false), 2, &mut r);
    assert_eq!(r.actions, vec![HostAction::ToggleGameButtons]);
    assert!(r.events.is_empty(), "the key does not reach the game");
    m.set_text_focus(Some((5, "")), &mut r);
    m.handle(h(true, false), 4, &mut r);
    assert_eq!(r.actions, vec![HostAction::ToggleGameButtons, HostAction::StartTextInput]);
}
