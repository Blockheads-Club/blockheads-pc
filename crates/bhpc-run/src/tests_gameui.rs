//! The game's on-screen controls as the host sees them, on a fake guest memory laid out like the game's objects.

use crate::gameui::*;
use bhpc_mem::GuestMemory;

const BASE: u32 = 0x4000_0000;
const VIEW: u32 = BASE + 0x1000;
const WORLD: u32 = BASE + 0x2000;
const UI: u32 = BASE + 0x3000;
const PAUSE: u32 = BASE + 0x4000;
const DPAD: u32 = BASE + 0x5000;
const WINFO: u32 = BASE + 0x6000;

/// Ivar offsets as the exports hold them (any distinct values do).
fn ivars() -> Ivars {
    Ivars {
        gv_world: 0x18,
        gv_pinch: 0x98,
        gv_window: 0x1c0,
        world_ui: 0xf0,
        tap_mv: 0x140,
        tap_pj: 0x100,
        tap_vp: 0x180,
        ui_pause: 0x40,
        ui_dpad: 0x44,
        view_displayed: 0x30,
        dpad_displayed: 0x78,
        dpad_window: 0x70,
        dpad_right: 0xa0,
    }
}

fn memory() -> GuestMemory {
    let m = GuestMemory::new().unwrap();
    m.map(BASE, 0x8000).unwrap();
    m
}

/// A game view in a world whose camera looks at tile (100.5, 50.5) from 48.3 tiles (tiles are 20 px on 1000x800).
fn in_world(m: &GuestMemory) -> GameUi {
    let iv = ivars();
    let w = |a: u32, v: u32| m.write_u32(a, v).unwrap();
    w(VIEW + iv.gv_world, WORLD);
    m.write_u64(VIEW + iv.gv_pinch, 1.0f64.to_bits()).unwrap();
    w(WORLD + iv.world_ui, UI);
    let mut mv = [0f32; 16];
    (mv[0], mv[5], mv[10], mv[15], mv[12], mv[13], mv[14]) = (1.0, 1.0, 1.0, 1.0, -100.5, -50.5, -48.284_27);
    let f = 2.414_213_5f32;
    let mut pj = [0f32; 16];
    (pj[0], pj[5], pj[10], pj[11], pj[14]) = (f / 1.25, f, -1.002, -1.0, -2.002);
    for i in 0..16 {
        w(WORLD + iv.tap_mv + 4 * i, mv[i as usize].to_bits());
        w(WORLD + iv.tap_pj + 4 * i, pj[i as usize].to_bits());
    }
    for (i, v) in [0u32, 0, 1000, 800].into_iter().enumerate() {
        w(WORLD + iv.tap_vp + 4 * i as u32, v);
    }
    w(UI + iv.ui_pause, PAUSE);
    w(UI + iv.ui_dpad, DPAD);
    w(DPAD + iv.dpad_window, WINFO);
    w(WINFO, 1024f32.to_bits());
    w(WINFO + 4, 768f32.to_bits());
    let g = GameUi::new(iv);
    g.saw_game_view(VIEW);
    g
}

#[test]
fn ivar_offsets_come_from_the_exports() {
    let m = memory();
    let export = |n: &str| IVAR_NAMES.iter().position(|x| format!("OBJC_IVAR_$_{x}") == n).map(|i| BASE + 4 * i as u32);
    for i in 0..IVAR_NAMES.len() as u32 {
        m.write_u32(BASE + 4 * i, 0x10 + i).unwrap();
    }
    let iv = Ivars::read(&m, &export).unwrap();
    assert_eq!((iv.gv_world, iv.gv_pinch, iv.gv_window, iv.dpad_window, iv.dpad_right), (0x10, 0x11, 0x12, 0x1b, 0x1c));
    assert_eq!(Ivars::read(&m, &|_| None).unwrap_err(), "no ivar GameView.world");
    let e = Ivars::read(&m, &|_| Some(0x10)).unwrap_err();
    assert!(e.starts_with("ivar GameView.world:"), "{e}");
}

#[test]
fn hiding_toggles_and_the_dpad_keeps_the_walking_finger() {
    let g = GameUi::default();
    assert!(!g.hidden() && !g.suppress(false));
    assert!(g.toggle());
    assert!(g.hidden() && g.suppress(false) && g.suppress(true));
    g.set_walking(true);
    assert!(g.suppress(false) && !g.suppress(true));
    assert!(!g.toggle());
    assert!(!g.suppress(true));
    use bhpc_input::GuestEvent as E;
    let g = GameUi::default();
    g.toggle();
    g.track_walk(&E::TouchBegin { id: 2, x: 0.0, y: 0.0, time_ms: 0 });
    assert!(!g.suppress(true));
    g.track_walk(&E::TouchEnd { id: 2, x: 0.0, y: 0.0, time_ms: 0 });
    assert!(g.suppress(true));
    g.track_walk(&E::TouchBegin { id: 2, x: 0.0, y: 0.0, time_ms: 0 });
    g.track_walk(&E::TouchCancel { ids: vec![0, 2], xs: vec![0.0; 2], ys: vec![0.0; 2], time_ms: 0 });
    assert!(g.suppress(true));
    g.track_walk(&E::TouchBegin { id: 0, x: 0.0, y: 0.0, time_ms: 0 });
    g.track_walk(&E::TouchCancel { ids: vec![0], xs: vec![0.0], ys: vec![0.0], time_ms: 0 });
    assert!(g.suppress(true));
    assert_eq!(message(true), "GAME BUTTONS HIDDEN");
    assert_eq!(message(false), "GAME BUTTONS SHOWN");
}

#[test]
fn the_dpad_can_stay_hidden_and_the_buttons_can_start_hidden() {
    // The D-pad alone: the other buttons show, the D-pad neither draws nor takes touches, except the walking finger.
    let g = GameUi::default().start_hidden(false, true);
    assert!(!g.hidden() && !g.off(false) && g.off(true));
    assert!(!g.suppress(false) && g.suppress(true));
    g.set_walking(true);
    assert!(!g.suppress(true) && g.off(true), "the walking finger reaches the D-pad, which still does not draw");
    g.set_walking(false);
    // Toggling hides the rest too; showing again leaves the D-pad hidden.
    assert!(g.toggle() && g.off(false) && g.off(true));
    assert!(!g.toggle() && !g.off(false) && g.off(true));
    // All hidden at the start, the D-pad with them until the buttons are shown.
    let g = GameUi::default().start_hidden(true, false);
    assert!(g.hidden() && g.off(false) && g.off(true) && g.suppress(true));
    assert!(!g.toggle() && !g.off(true) && !g.suppress(true));
    assert_eq!(message(false), "GAME BUTTONS SHOWN");
}

#[test]
fn objects_grid_pause_and_dpad_of_a_world() {
    let m = memory();
    // No game view yet, or one without a world: nothing.
    let none = GameUi::new(ivars());
    assert_eq!((none.game_view(), none.world(&m)), (None, None));
    assert!(none.grid(&m, (1000.0, 800.0)).is_none() && !none.pause_shown(&m) && none.dpad(&m).is_none());
    assert_eq!(none.zoom(&m), None);
    assert!(!none.set_zoom(&m, 2.0));
    let g = in_world(&m);
    assert_eq!(g.world(&m), Some(WORLD));
    let grid = g.grid(&m, (1000.0, 800.0)).unwrap();
    let c = grid.screen((100.5, 50.5)).unwrap();
    assert!((c.0 - 500.0).abs() < 0.05 && (c.1 - 400.0).abs() < 0.05, "{c:?}");
    assert!((grid.tile_size(c) - 20.0).abs() < 0.2);
    // Pause menu.
    assert!(!g.pause_shown(&m));
    m.write_u8(PAUSE + ivars().view_displayed, 1).unwrap();
    assert!(g.pause_shown(&m));
    // D-pad: only while it shows.
    assert_eq!(g.dpad(&m), None);
    m.write_u8(DPAD + ivars().dpad_displayed, 1).unwrap();
    assert_eq!(g.dpad(&m), Some(((1024.0, 768.0), false)));
    m.write_u8(DPAD + ivars().dpad_right, 1).unwrap();
    assert_eq!(g.dpad(&m), Some(((1024.0, 768.0), true)));
    // Zoom.
    assert_eq!(g.zoom(&m), Some(1.0));
    assert!(g.set_zoom(&m, 2.0));
    assert_eq!(g.zoom(&m), Some(2.0));
    m.write_u64(VIEW + ivars().gv_pinch, f64::NAN.to_bits()).unwrap();
    assert_eq!(g.zoom(&m), None);
    // View size: only a positive one counts.
    assert_eq!(none.view_size(&m), None);
    assert_eq!(g.view_size(&m), None);
    m.write_u32(VIEW + ivars().gv_window, 512f32.to_bits()).unwrap();
    assert_eq!(g.view_size(&m), None);
    m.write_u32(VIEW + ivars().gv_window + 4, 384f32.to_bits()).unwrap();
    assert_eq!(g.view_size(&m), Some((512.0, 384.0)));
    // A view outside mapped memory: every read fails softly.
    g.saw_game_view(0x10);
    assert_eq!(g.world(&m), None);
    assert!(g.grid(&m, (1.0, 1.0)).is_none());
}

#[test]
fn unreadable_parts_of_a_world_fail_softly() {
    let m = memory();
    let g = in_world(&m);
    let iv = ivars();
    // A projection that cannot be read, a viewport that cannot be read.
    let far = 0x7fff_0000u32.wrapping_sub(WORLD);
    let bad = |f: &dyn Fn(&mut Ivars)| {
        let mut v = iv;
        f(&mut v);
        let g2 = GameUi::new(v);
        g2.saw_game_view(VIEW);
        g2
    };
    assert!(bad(&|v| v.tap_pj = far).grid(&m, (1000.0, 800.0)).is_none());
    assert!(bad(&|v| v.tap_mv = far).grid(&m, (1000.0, 800.0)).is_none());
    assert!(bad(&|v| v.tap_vp = far).grid(&m, (1000.0, 800.0)).is_none());
    // A D-pad whose window info is missing or unreadable.
    m.write_u8(DPAD + iv.dpad_displayed, 1).unwrap();
    m.write_u32(DPAD + iv.dpad_window, 0).unwrap();
    assert_eq!(g.dpad(&m), None);
    m.write_u32(DPAD + iv.dpad_window, BASE + 0x7ffc).unwrap();
    assert_eq!(g.dpad(&m), None);
}

#[test]
fn zoom_steps_and_levels() {
    assert_eq!(zoom_cycle(1.0), 2.0);
    assert_eq!(zoom_cycle(2.0), 3.0);
    assert_eq!(zoom_cycle(3.0), 0.5);
    assert_eq!(zoom_cycle(0.5), 1.0);
    assert_eq!(zoom_cycle(1.1), 2.0, "from where the zoom settles");
    assert_eq!(zoom_cycle(1.5), 2.0, "a level outside the cycle: after the nearest");
    assert_eq!(zoom_cycle(8.0), 0.5);
    assert_eq!(zoom_cycle(f64::NAN), 2.0);
    assert!(ZOOM_LEVELS.iter().all(|l| SNAP_LEVELS.contains(l)));
}

#[test]
fn the_dpad_centre_in_content_pixels() {
    assert_eq!(dpad_centre((1024.0, 768.0), (1024.0, 768.0), false, 120.0), Some((120.0, 648.0)));
    assert_eq!(dpad_centre((1024.0, 768.0), (2048.0, 1536.0), true, 120.0), Some((1808.0, 1296.0)));
    assert_eq!(dpad_centre((0.0, 768.0), (1024.0, 768.0), false, 120.0), None);
}

#[test]
fn wheel_steps_go_through_the_games_own_levels() {
    // Where the game settles.
    let settles =
        [(0.3, 0), (0.59, 0), (0.6, 1), (0.8, 1), (1.24, 1), (1.25, 2), (1.8, 3), (2.6, 4), (5.0, 5), (20.0, 5)];
    for (z, i) in settles {
        assert_eq!(snap_level(z), i, "{z}");
    }
    assert_eq!(snap_level(f64::NAN), 1);
    // One level per notch, from the level a zoom settles on, within the levels.
    assert_eq!(wheel_target(1.0, 1), 0.5);
    assert_eq!(wheel_target(1.0, -1), 1.5);
    assert_eq!(wheel_target(1.4, -1), 2.0, "from the level a running step heads to");
    assert_eq!(wheel_target(1.0, -3), 3.0);
    assert_eq!(wheel_target(0.5, 1), 0.5);
    assert_eq!(wheel_target(8.0, -100), 8.0);
    assert_eq!(wheel_target(8.0, 100), 0.5);
    // The pinch centre in view points.
    assert_eq!(view_point((512, 384), (1024.0, 768.0), (512.0, 384.0)), Some((256.0, 192.0)));
    assert_eq!(view_point((10, 20), (100.0, 100.0), (100.0, 50.0)), Some((10.0, 10.0)));
    assert_eq!(view_point((1, 1), (0.0, 1.0), (1.0, 1.0)), None);
    assert_eq!(view_point((1, 1), (1.0, f32::NAN), (1.0, 1.0)), None);
    assert_eq!(view_point((1, 1), (1.0, 1.0), (-1.0, 1.0)), None);
    assert_eq!(view_point((1, 1), (1.0, 1.0), (1.0, 0.0)), None);
}

#[test]
fn a_wheel_step_is_a_short_eased_pinch() {
    let a = WheelAnim { base: 1.0, from: 1.0, to: 2.0, start_ms: 100, centre: (5.0, 6.0) };
    assert_eq!(a.at(50), (1.0, false));
    assert_eq!(a.at(100), (1.0, false));
    let (mid, done) = a.at(100 + WHEEL_ZOOM_MS / 2);
    assert!(!done && (mid - 2f64.powf(0.75)).abs() < 1e-9, "{mid}");
    assert_eq!(a.at(100 + WHEEL_ZOOM_MS), (2.0, true));
    assert_eq!(a.at(10_000), (2.0, true));
    assert_eq!(a.factor(2.0), 0.5);
    assert_eq!(a.factor(0.5), 2.0);
    let later = WheelAnim { from: 1.5, to: 3.0, ..a };
    assert_eq!(later.at(100), (1.5, false));
    assert_eq!(later.factor(3.0), 1.0 / 3.0, "the factor stays relative to where the pinch began");
    assert!(a.refused(1.5, 1.0));
    assert!(!a.refused(1.5, 1.4));
    assert!(!a.refused(1.000_01, 1.0), "nothing asked yet");
    let (x, y, nan) = (5f32.to_bits(), 6f32.to_bits(), f32::NAN.to_bits());
    assert_eq!(pinch_args(7, 9, PINCH_BEGAN, 1.0, a.centre), [7, 9, 1f32.to_bits(), nan, 1, x, y, 2]);
    assert_eq!(pinch_args(7, 9, PINCH_CHANGED, 0.5, a.centre), [7, 9, 0.5f32.to_bits(), nan, 2, x, y, 2]);
    assert_eq!(PINCH_ENDED, 3);
    assert_eq!(PINCH_SELECTOR.matches(':').count(), 5);
}

#[test]
fn a_reversal_during_or_right_after_a_step_counts_from_the_latest_target() {
    let c = Some((5.0, 6.0));
    // Out from 1: no step yet, so a pinch begins.
    let mut anim = None;
    let to = wheel_target(zoom_base(anim.as_ref(), 1.0), -1);
    assert_eq!(to, 1.5);
    assert_eq!(plan_zoom(&mut anim, 1.0, to, 0, c, true), ZoomPlan::Begin((5.0, 6.0)));
    anim = Some(WheelAnim { base: 1.0, from: 1.0, to, start_ms: 0, centre: (5.0, 6.0) });
    // In 50 ms later, mid-step at 1.26: back to 1, from the step's target, re-aiming the running pinch.
    let to = wheel_target(zoom_base(anim.as_ref(), 1.26), 1);
    assert_eq!(to, 1.0);
    assert_eq!(plan_zoom(&mut anim, 1.26, to, 50, c, true), ZoomPlan::Retarget);
    let a = anim.unwrap();
    assert_eq!((a.from, a.to, a.start_ms, a.base), (1.26, 1.0, 50, 1.0));
    assert_eq!(a.at(50 + WHEEL_ZOOM_MS), (1.0, true));
    // Out, out, in within one step: 1 -> 1.5 -> 2 -> 1.5.
    let mut anim = Some(WheelAnim { base: 1.0, from: 1.0, to: 1.5, start_ms: 0, centre: (5.0, 6.0) });
    for (z, steps, want) in [(1.1, -1, 2.0), (1.3, 1, 1.5)] {
        let to = wheel_target(zoom_base(anim.as_ref(), z), steps);
        assert_eq!(to, want);
        assert_eq!(plan_zoom(&mut anim, z, to, 10, c, true), ZoomPlan::Retarget);
    }
    assert_eq!(anim.unwrap().to, 1.5);
    // Right after a step: it starts from the level shown; no handler or no centre writes; nothing to do writes.
    let mut none = None;
    assert_eq!(plan_zoom(&mut none, 1.5, 1.0, 0, c, true), ZoomPlan::Begin((5.0, 6.0)));
    assert_eq!(plan_zoom(&mut none, 1.5, 1.0, 0, c, false), ZoomPlan::Write);
    assert_eq!(plan_zoom(&mut none, 1.5, 1.0, 0, None, true), ZoomPlan::Write);
    assert_eq!(plan_zoom(&mut none, 1.5, 1.5, 0, c, true), ZoomPlan::Write);
    // A step the game dropped still lands.
    let a = WheelAnim { base: 1.0, from: 1.0, to: 1.5, start_ms: 0, centre: (5.0, 6.0) };
    assert!(a.missed(1.0));
    assert!(!a.missed(1.4999));
}
