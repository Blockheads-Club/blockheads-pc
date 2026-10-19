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
    assert_eq!((iv.gv_world, iv.gv_pinch, iv.dpad_window, iv.dpad_right), (0x10, 0x11, 0x1a, 0x1b));
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
    assert_eq!(zoom_step(1.0, true), 0.8);
    assert_eq!(zoom_step(1.0, false), 1.25);
    assert_eq!(zoom_step(0.55, true), ZOOM_MIN);
    assert_eq!(zoom_step(3.9, false), ZOOM_MAX);
    assert_eq!(zoom_step(f64::NAN, false), 1.25);
    assert_eq!(zoom_step(-1.0, true), 0.8);
    assert_eq!(zoom_cycle(1.0), 2.0);
    assert_eq!(zoom_cycle(2.0), 4.0);
    assert_eq!(zoom_cycle(4.0), 0.5);
    assert_eq!(zoom_cycle(0.5), 1.0);
    assert_eq!(zoom_cycle(1.3), 2.0, "from between levels: after the nearest");
    assert_eq!(zoom_cycle(f64::NAN), 2.0);
}

#[test]
fn the_dpad_centre_in_content_pixels() {
    assert_eq!(dpad_centre((1024.0, 768.0), (1024.0, 768.0), false, 120.0), Some((120.0, 648.0)));
    assert_eq!(dpad_centre((1024.0, 768.0), (2048.0, 1536.0), true, 120.0), Some((1808.0, 1296.0)));
    assert_eq!(dpad_centre((0.0, 768.0), (1024.0, 768.0), false, 120.0), None);
}
