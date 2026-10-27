use crate::embed::*;
use bhpc_app::{Event, Key, Mods, MouseButton};
use bhpc_input::{HostEvent, HostKey, Input, Trigger};
use bhpc_launcher::embed::Cmd;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn flags_default_to_the_plain_desktop_window() {
    let f = parse_play_flags(&s(&["--apk", "x.apk"])).unwrap();
    assert_eq!(
        f,
        PlayFlags {
            fullscreen: false,
            vsync: true,
            title: None,
            bindings: bhpc_input::Bindings::default(),
            embed: None,
            fps_limit: None,
            show_fps: false,
            fps_position: bhpc_app::hud::Corner::TopLeft,
            unlock_fps: false,
            game_speed: 1.0,
            zoom: crate::aspect::GameZoom::Tablet,
            ui_scale: 1.0,
            controller: true,
            hide_dpad: false,
            pad_tile_cursor: false,
            lang: bhpc_app::hud::Lang::En
        }
    );
}

#[test]
fn flags_carry_the_launcher_settings() {
    let f = parse_play_flags(&s(&[
        "--fullscreen",
        "--no-vsync",
        "--fps-limit",
        "30",
        "--show-fps",
        "--unlock-fps",
        "--game-speed",
        "1.5",
        "--fps-position",
        "br",
        "--title",
        "Blockheads PC · Playing",
        "--embed",
        "/run/f",
        "--bind",
        "touch=Middle",
        "--bind",
        "back=Q",
        // The config's value comes later and loses against the first flag for the same action.
        "--bind",
        "touch=Right",
    ]))
    .unwrap();
    assert!(f.fullscreen && !f.vsync && f.show_fps && f.unlock_fps);
    assert!(!parse_play_flags(&s(&["--no-controller"])).unwrap().controller);
    assert!(parse_play_flags(&s(&["--hide-dpad"])).unwrap().hide_dpad);
    assert!(parse_play_flags(&s(&["--pad-tile-cursor"])).unwrap().pad_tile_cursor);
    assert_eq!(parse_play_flags(&s(&["--lang", "kk"])).unwrap().lang, bhpc_app::hud::Lang::Kk);
    assert!(parse_play_flags(&s(&["--lang"])).unwrap_err().contains("--lang"));
    assert_eq!(f.game_speed, 1.5);
    assert!(parse_play_flags(&s(&["--game-speed", "9"])).unwrap_err().contains("0.25 to 4"));
    assert!(parse_play_flags(&s(&["--game-speed"])).is_err());
    assert_eq!(parse_play_flags(&s(&["--zoom", "tablet"])).unwrap().zoom, crate::aspect::GameZoom::Tablet);
    assert!(parse_play_flags(&s(&["--zoom", "huge"])).unwrap_err().contains("phone, close or tablet"));
    assert!(parse_play_flags(&s(&["--zoom"])).is_err());
    assert_eq!(parse_play_flags(&s(&["--ui-scale", "1.15"])).unwrap().ui_scale, 1.15);
    assert!(parse_play_flags(&s(&["--ui-scale", "3"])).unwrap_err().contains("from 0.8 to 1.3"));
    assert!(parse_play_flags(&s(&["--ui-scale"])).is_err());
    assert_eq!(f.fps_limit, Some(30));
    assert_eq!(f.fps_position, bhpc_app::hud::Corner::BottomRight);
    // The first --fps-limit wins (explicit flags come before the config's); 0 means no cap.
    assert_eq!(parse_play_flags(&s(&["--fps-limit", "0", "--fps-limit", "60"])).unwrap().fps_limit, None);
    assert_eq!(f.title.as_deref(), Some("Blockheads PC · Playing"));
    assert_eq!(f.embed, Some("/run/f".into()));
    assert_eq!(f.bindings.touch.input, Input::Mouse(bhpc_input::MouseButton::Middle));
    assert_eq!(f.bindings.back, Trigger::plain(Input::Key(HostKey::Char('q'))));
}

#[test]
fn bad_flags_are_reported() {
    assert!(parse_play_flags(&s(&["--bind", "jump=Q"])).unwrap_err().contains("unknown action"));
    assert!(parse_play_flags(&s(&["--bind"])).is_ok(), "a dangling --bind has nothing to apply");
    assert!(parse_play_flags(&s(&["--title"])).unwrap_err().contains("--title"));
    assert!(parse_play_flags(&s(&["--embed"])).unwrap_err().contains("--embed"));
    assert!(parse_play_flags(&s(&["--fps-limit"])).unwrap_err().contains("--fps-limit"));
    assert!(parse_play_flags(&s(&["--fps-limit", "fast"])).unwrap_err().contains("--fps-limit"));
    assert!(parse_play_flags(&s(&["--fps-position", "middle"])).unwrap_err().contains("--fps-position"));
    assert!(parse_play_flags(&s(&["--fps-position"])).unwrap_err().contains("--fps-position"));
}

#[test]
fn commands_become_window_events() {
    let ev = |c: Cmd| cmd_event(&c);
    assert_eq!(ev(Cmd::Size { width: 1, height: 1 }), None);
    assert_eq!(ev(Cmd::Quit), None);
    assert_eq!(ev(Cmd::ModsReload), None);
    assert_eq!(ev(Cmd::ModsChat("/x".into())), None);
    assert_eq!(ev(Cmd::Layout(bhpc_launcher::embed::Layout::GlRgba)), None);
    assert_eq!(ev(Cmd::Move { x: 1.0, y: 2.0 }), Some(Event::MouseMove { x: 1.0, y: 2.0 }));
    for (n, b) in
        [(0, MouseButton::Left), (1, MouseButton::Middle), (2, MouseButton::Right), (4, MouseButton::Other(4))]
    {
        assert_eq!(
            ev(Cmd::Button { button: n, pressed: true, x: 3.0, y: 4.0, mods: 0 }),
            Some(Event::MouseButton { button: b, pressed: true, x: 3.0, y: 4.0 })
        );
    }
    assert_eq!(ev(Cmd::Wheel { dx: 0.5, dy: -1.0, x: 0.0, y: 0.0, mods: 0 }), Some(Event::Wheel { dx: 0.5, dy: -1.0 }));
    assert_eq!(
        ev(Cmd::Key { hid: 68, pressed: true, repeat: true, mods: 7 }),
        Some(Event::Key {
            key: Key(68),
            pressed: true,
            repeat: true,
            mods: Mods { shift: true, ctrl: true, alt: true }
        })
    );
    assert_eq!(ev(Cmd::Text("é".into())), Some(Event::Text("é".into())));
    assert_eq!(ev(Cmd::Focus(true)), Some(Event::Focus(true)));
    assert_eq!(ev(Cmd::Visible(false)), Some(Event::Visible(false)));
    let m = size_metrics(1280, 720);
    assert_eq!((m.logical, m.drawable), ((1280, 720), (1280, 720)));
}

#[test]
fn the_tracker_adds_modifiers_and_the_pointer_position() {
    let mut t = InputTracker::default();
    let key = |hid, pressed| Event::Key { key: Key(hid), pressed, repeat: false, mods: Mods::default() };
    let click = |pressed| Event::MouseButton { button: MouseButton::Left, pressed, x: 5.0, y: 6.0 };
    let pressed = |ev: Option<HostEvent>| match ev {
        Some(HostEvent::MouseButton { mods, .. }) => mods,
        other => panic!("{other:?}"),
    };
    assert_eq!(pressed(t.map(&click(true))), bhpc_input::Mods::default());
    t.map(&key(224, true));
    t.map(&key(229, true));
    t.map(&key(226, true));
    assert_eq!(t.mods(), bhpc_input::Mods { ctrl: true, shift: true, alt: true });
    assert_eq!(pressed(t.map(&click(true))), t.mods());
    t.map(&Event::MouseMove { x: 40.0, y: 50.0 });
    assert_eq!(
        t.map(&Event::Wheel { dx: 0.0, dy: 1.5 }),
        Some(HostEvent::Wheel { dy: 1.5, x: 40.0, y: 50.0, mods: t.mods() })
    );
    for hid in [228, 225, 230] {
        t.map(&key(hid, false));
    }
    assert_eq!(t.mods(), bhpc_input::Mods::default());
    // Other keys and events with no mapper counterpart pass through or vanish.
    assert_eq!(
        t.map(&key(4, true)),
        Some(HostEvent::Key {
            key: HostKey::Char('a'),
            pressed: true,
            repeat: false,
            mods: bhpc_input::Mods::default()
        })
    );
    assert_eq!(t.map(&Event::Fullscreen(true)), None);
}

#[test]
fn frames_are_flipped_and_swizzled() {
    // 2x2: bottom row first in the read-back (red, green), top row (blue, white).
    let rgba = [255, 0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 7, 255, 255, 255, 7];
    let mut out = vec![9; 3];
    rgba_bottom_up_to_bgra(2, 2, &rgba, &mut out);
    assert_eq!(out, [255, 0, 0, 255, 255, 255, 255, 255, 0, 0, 255, 255, 0, 255, 0, 255]);
}

#[test]
fn frames_go_out_in_the_layout_the_view_wants() {
    use bhpc_launcher::embed::Layout;
    let rgba = [255, 0, 0, 0, 0, 255, 0, 0];
    let mut scratch = Vec::new();
    assert_eq!(frame_pixels(Layout::GlRgba, 1, 2, &rgba, &mut scratch), rgba, "a GL view takes the read-back as is");
    assert!(scratch.is_empty());
    assert_eq!(frame_pixels(Layout::Bgra, 1, 2, &rgba, &mut scratch), [0, 255, 0, 255, 0, 0, 255, 255]);
}

#[test]
fn the_asynchronous_read_back_has_an_off_switch() {
    assert!(async_readback(None));
    assert!(async_readback(Some("1")));
    assert!(!async_readback(Some("0")));
}

#[test]
fn frame_pacer_sends_about_one_frame_per_display_period() {
    use crate::embed::FramePacer;
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let ms = |m: f64| t0 + Duration::from_secs_f64(m / 1000.0);
    // 80 frames a second on a 60 Hz display: about 60 go out, not every other one.
    let mut p = FramePacer::new(Some(60));
    let sent = (0..80).filter(|&i| p.take(ms(f64::from(i) * 12.5))).count();
    assert!((58..=62).contains(&sent), "{sent} of 80");
    // A slower game sends all of its frames.
    let mut p = FramePacer::new(Some(60));
    assert_eq!((0..30).filter(|&i| p.take(ms(f64::from(i) * 33.3))).count(), 30);
    // After a stall the next frame goes, then the pace is back (no burst).
    let mut p = FramePacer::new(Some(60));
    assert!(p.take(ms(0.0)));
    assert!(p.take(ms(500.0)));
    assert!(!p.take(ms(505.0)));
    assert!(p.take(ms(517.0)));
    // Unknown or implausible rates mean 60 Hz.
    for hz in [None, Some(0), Some(10_000)] {
        let mut p = FramePacer::new(hz);
        assert!(p.take(ms(0.0)));
        assert!(!p.take(ms(10.0)), "{hz:?}");
        assert!(p.take(ms(15.0)), "{hz:?}");
    }
    // A 144 Hz display takes frames 7 ms apart.
    let mut p = FramePacer::new(Some(144));
    assert!(p.take(ms(0.0)));
    assert!(p.take(ms(7.0)));
}

#[test]
fn transition_gate_holds_frames_of_the_old_layout_for_a_while() {
    use crate::embed::{TransitionGate, TRANSITION_HOLD};
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut g = TransitionGate::default();
    // The first size is not a change, whatever the surface.
    assert!(!g.hold((1100, 676), (1100, 676), at(0)));
    assert!(!g.hold((1100, 676), (800, 676), at(5)));
    // Wider window, the game still on the old aspect: held; once it matches, frames flow and stay flowing.
    assert!(g.hold((1920, 948), (1100, 676), at(10)));
    assert!(g.hold((1920, 948), (1100, 676), at(100)));
    assert!(!g.hold((1920, 948), (1369, 676), at(150)));
    assert!(!g.hold((1920, 948), (1100, 676), at(160)));
    // A screen that keeps the old aspect is shown again after the hold.
    assert!(g.hold((1200, 676), (1369, 676), at(200)));
    assert!(!g.hold((1200, 676), (1369, 676), at(200) + TRANSITION_HOLD));
    assert!(!g.hold((1200, 676), (1369, 676), at(900)));
}

#[test]
fn the_in_window_game_is_capped_at_the_display_rate() {
    use crate::embed::embedded_fps_limit;
    assert_eq!(embedded_fps_limit(None, Some(144)), 144);
    assert_eq!(embedded_fps_limit(Some(0), Some(120)), 120);
    assert_eq!(embedded_fps_limit(Some(30), Some(120)), 30);
    assert_eq!(embedded_fps_limit(Some(240), Some(120)), 120);
    assert_eq!(embedded_fps_limit(None, None), 60);
    assert_eq!(embedded_fps_limit(None, Some(1)), 60);
}
