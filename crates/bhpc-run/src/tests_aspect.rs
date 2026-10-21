//! The game's logical screen size for any window aspect.

use crate::aspect::*;

const P: GameZoom = GameZoom::Phone;
const C: GameZoom = GameZoom::Close;
const T: GameZoom = GameZoom::Tablet;

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.1 && (a.1 - b.1).abs() < 0.1
}

#[test]
fn the_phone_zoom_gives_a_phone_s_points() {
    // 16:9 at any size: 667x375 points, what the game gets on a 1920x1080 Android phone (an iPhone 6 at 2x).
    for (w, h) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
        assert!(near(points(w, h, P, 1.0), (666.7, 375.0)), "{w}x{h}: {:?}", points(w, h, P, 1.0));
    }
    // 1280x720 was 2.25 pixels per point (568x320, an iPhone 5): now 1.92, 17 % more world each way.
    assert_eq!(ui_scale(1280, 720, P, 1.0), 1.92);
    // 20:9 like a 2400x1080 phone (812x375 there, an iPhone X at 3x), 16:10 and tall windows: the short side is the
    // phone's, the long side follows the aspect.
    assert!(near(points(2400, 1080, P, 1.0), (833.3, 375.0)));
    assert!(near(points(1280, 800, P, 1.0), (600.0, 375.0)));
    assert!(near(points(720, 1280, P, 1.0), (375.0, 666.7)));
    // 4:3 keeps the phone layout; too square for a phone: the iPad's points.
    assert!(near(points(1024, 768, P, 1.0), (500.0, 375.0)));
    assert_eq!(points(768, 768, P, 1.0), (1024.0, 1024.0));
    // Small windows go under one pixel per point; an empty display gets 1.
    assert!(ui_scale(640, 360, P, 1.0) < 1.0);
    assert_eq!(ui_scale(0, 0, P, 1.0), 1.0);
    assert_eq!(ui_scale(100, -5, P, 1.0), 1.0);
}

#[test]
fn the_close_zoom_keeps_at_least_an_iphone_5_of_points() {
    // 16:9 keeps the iPhone 5's 568x320 points (the old default).
    assert_eq!(points(1136, 640, C, 1.0), (568.0, 320.0));
    let (w, h) = points(1920, 1080, C, 1.0);
    assert!((w - 568.9).abs() < 0.1 && h == 320.0, "{w}x{h}");
    // 16:10: the long side stays 568 points and the short side grows.
    assert_eq!(points(1280, 800, C, 1.0), (568.0, 355.0));
    // 21:9: the short side stays 320 points, the long side grows.
    assert_eq!(points(2560, 1080, C, 1.0), (2560.0 / 3.375, 320.0));
    // A tall window: the same rule with the sides swapped.
    assert_eq!(points(800, 1280, C, 1.0), (355.0, 568.0));
    // Below the minimum the scale drops under 1 and the points stay.
    assert_eq!(ui_scale(568, 320, C, 1.0), 1.0);
    assert_eq!(ui_scale(284, 160, C, 1.0), 0.5);
    assert_eq!(ui_scale(0, 0, C, 1.0), 1.0);
}

#[test]
fn squarer_windows_stay_below_the_ipad_layout() {
    // 4:3 and 5:4: the short side stops at 414 points (the game's iPad layout starts at 415).
    let (w, h) = points(1024, 768, C, 1.0);
    assert!((w - 552.0).abs() < 0.01 && (h - 414.0).abs() < 0.01, "{w}x{h}");
    let (w, h) = points(1280, 1024, C, 1.0);
    assert!((w - 517.5).abs() < 0.01 && (h - 414.0).abs() < 0.01, "{w}x{h}");
    // Too square for a phone: the iPad's points.
    assert_eq!(points(1024, 1000, C, 1.0).0, 1024.0);
    assert_eq!(points(768, 768, C, 1.0), (1024.0, 1024.0));
}

#[test]
fn the_tablet_zoom_gives_an_ipad_s_points() {
    assert_eq!(points(1024, 768, T, 1.0), (1024.0, 768.0));
    assert!(near(points(1920, 1080, T, 1.0), (1365.3, 768.0)));
    assert!(near(points(1280, 720, T, 1.0), (1365.3, 768.0)));
    assert_eq!(ui_scale(0, 0, T, 1.0), 1.0);
}

#[test]
fn only_the_best_emulated_mode_becomes_native() {
    assert_eq!(screen_mode(MODE_BEST_EMULATED), MODE_NATIVE);
    for m in [0, 5, 12, 13, 14] {
        assert_eq!(screen_mode(m), m);
    }
}

#[test]
fn only_init_calls_from_the_native_mode_are_rescaled() {
    assert!(called_from_native_mode(0x1000 + 0xd4, 0x1000));
    assert!(!called_from_native_mode(0x1000 + NATIVE_MODE_LEN, 0x1000));
    assert!(!called_from_native_mode(0xfff, 0x1000));
}

#[test]
fn a_fullscreen_start_uses_the_desktop_in_drawable_pixels() {
    assert_eq!(drawable_for((2560, 1600), (1024, 768), (1024, 768)), (2560, 1600));
    assert_eq!(drawable_for((1440, 900), (1024, 768), (2048, 1536)), (2880, 1800));
    assert_eq!(drawable_for((1440, 900), (0, 0), (0, 0)), (1440, 900));
    assert_eq!(drawable_for((1440, 900), (1024, 768), (0, 768)), (1440, 900));
}

#[test]
fn later_modes_keep_the_first_scale_and_the_fresh_points() {
    // The first scale for its own display: the display's pixels.
    assert_eq!(content_size(1280, 800, ui_scale(1280, 800, C, 1.0), C, 1.0), (1280.0, 800.0));
    assert_eq!(content_size(1280, 800, ui_scale(1280, 800, P, 1.0), P, 1.0), (1280.0, 800.0));
    // Started at 1280x800 (2.25 pixels per point), then fullscreen on a 2560x1600 screen: the same 568x355 points,
    // drawn at 2.25 into 1280x800 pixels and stretched.
    assert_eq!(content_size(2560, 1600, ui_scale(1280, 800, C, 1.0), C, 1.0), (1280.0, 800.0));
    // Then 21:9: 758x320 points at 2.25 pixels per point.
    assert_eq!(content_size(2560, 1080, ui_scale(1280, 800, C, 1.0), C, 1.0), (1709.0, 721.0));
    // The phone zoom: 1280x720 (1.92 pixels per point), then 1920x1080: the same 667x375 points.
    assert_eq!(content_size(1920, 1080, ui_scale(1280, 720, P, 1.0), P, 1.0), (1280.0, 720.0));
    // A tiny window still gets one pixel.
    assert_eq!(content_size(1, 1, 0.001, P, 1.0), (1.0, 1.0));
}

#[test]
fn the_game_ui_scale_divides_the_zoom_s_points() {
    // Within the phone layout's edges: 1.1 is 10 % bigger, 0.95 5 % smaller.
    assert!(near(points(1280, 720, P, 1.1), (606.1, 340.9)));
    assert!(near(points(1280, 720, P, 0.95), (701.8, 394.7)));
    // 16:9 at the slider's ends: the phone layout stops at 736x414 (more is the iPad layout) and 568x320.
    assert!(near(points(1280, 720, P, 0.8), (736.0, 414.0)));
    assert_eq!(points(1280, 720, P, 1.3), (1280.0 / 2.25, 320.0));
    // The close zoom at 16:9 already sits at 320 points: bigger stays, smaller gives 400 (320 / 0.8); at 16:10 it
    // goes down to 320 points too.
    assert_eq!(points(1136, 640, C, 1.3), (568.0, 320.0));
    assert_eq!(points(1136, 640, C, 0.8), (710.0, 400.0));
    assert_eq!(points(1280, 800, C, 1.3), (512.0, 320.0));
    // 4:3 keeps the phone layout (480x360 to 552x414), and a bigger UI never ends with more points than the zoom's
    // own.
    assert!(near(points(1024, 768, P, 1.3), (480.0, 360.0)));
    assert!(near(points(1024, 768, P, 0.8), (552.0, 414.0)));
    assert!(near(points(1024, 768, C, 0.8), (552.0, 414.0)));
    assert!(near(points(1024, 768, C, 1.3), (480.0, 360.0)));
    // A window just wide enough for the phone layout (500x375): a bigger UI cannot make it narrower.
    assert!(near(points(960, 720, P, 1.3), (480.0, 360.0)));
    // The iPad layout takes the whole range (its short side stays at least 591 points).
    assert!(near(points(1280, 720, T, 1.3), (1050.3, 590.8)));
    assert!(near(points(1280, 720, T, 0.8), (1706.7, 960.0)));
    assert!(near(points(768, 768, P, 1.25), (819.2, 819.2)));
    assert_eq!(ui_scale(0, 0, P, 1.3), 1.0);
}

#[test]
fn the_game_ui_scale_flag_is_a_number_in_range() {
    assert_eq!(parse_fine("1.2"), Some(1.2));
    assert_eq!(parse_fine(" 0.8 "), Some(0.8));
    assert_eq!(parse_fine("1.3"), Some(1.3));
    for bad in ["0.5", "2", "big", ""] {
        assert_eq!(parse_fine(bad), None, "{bad}");
    }
}

#[test]
fn resizes_keep_the_game_ui_scale() {
    // Started at 1280x720 with 110 % (2.112 pixels per point), then 1920x1080: the same 606x341 points.
    let s = ui_scale(1280, 720, P, 1.1);
    assert_eq!(content_size(1920, 1080, s, P, 1.1), (1280.0, 720.0));
    // Then 4:3: its own 110 % points (480x360, the phone layout's narrowest), drawn at the first scale.
    let (w, h) = content_size(1440, 1080, s, P, 1.1);
    assert!((w / s - 480.0).abs() < 0.5 && (h / s - 360.0).abs() < 0.5, "{w}x{h}");
}
