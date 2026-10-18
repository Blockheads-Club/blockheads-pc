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
            embed: None
        }
    );
}

#[test]
fn flags_carry_the_launcher_settings() {
    let f = parse_play_flags(&s(&[
        "--fullscreen",
        "--no-vsync",
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
    assert!(f.fullscreen && !f.vsync);
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
}

#[test]
fn commands_become_window_events() {
    let ev = |c: Cmd| cmd_event(&c);
    assert_eq!(ev(Cmd::Size { width: 1, height: 1 }), None);
    assert_eq!(ev(Cmd::Quit), None);
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
