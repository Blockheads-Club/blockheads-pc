use crate::script::{build, click_events, click_line, key_code, parse, Action};
use bhpc_app::{Event, Key, Mods, MouseButton};

fn at(steps: &[(u64, Action)], i: usize) -> u64 {
    steps[i].0
}

#[test]
fn click_expands_to_move_press_release() {
    let s = parse("100 click 10 20\n").unwrap();
    assert_eq!(s.len(), 3);
    assert_eq!(s[0], (100, Action::Event(Event::MouseMove { x: 10.0, y: 20.0 })));
    assert_eq!(
        s[1],
        (160, Action::Event(Event::MouseButton { button: MouseButton::Left, pressed: true, x: 10.0, y: 20.0 }))
    );
    assert!(matches!(s[2], (220, Action::Event(Event::MouseButton { pressed: false, .. }))));
}

#[test]
fn relative_times_follow_the_previous_end() {
    let s = parse("# comment\n\n 1000 hold 5 6 500   # trailing\n+100 wait\n+0 shot menu\n").unwrap();
    // hold: move 1000, press 1060, release 1560; wait at 1660; shot at the same time
    assert_eq!(at(&s, 2), 1560);
    assert_eq!(s[3], (1660, Action::Shot("menu".into())));
}

#[test]
fn drag_interpolates_and_releases_at_the_end() {
    let s = parse("0 drag 0 0 100 50 90\n").unwrap();
    let moves: Vec<_> = s
        .iter()
        .filter_map(|(_, a)| match a {
            Action::Event(Event::MouseMove { x, y }) => Some((*x, *y)),
            _ => None,
        })
        .collect();
    assert_eq!(moves.first(), Some(&(0.0, 0.0)));
    assert_eq!(moves.last(), Some(&(100.0, 50.0)));
    assert_eq!(moves.len(), 1 + 3);
    let last = s.last().unwrap();
    assert!(
        matches!(last.1, Action::Event(Event::MouseButton { pressed: false, x, y, .. }) if x == 100.0 && y == 50.0)
    );
    // a zero-length drag still moves once
    assert_eq!(parse("0 drag 1 1 2 2 0\n").unwrap().len(), 4);
}

#[test]
fn keys_and_text() {
    let s = parse("0 key enter\n10 keydown left\n20 keyup left\n30 text hello  world \n40 quit\n50 mark in_world\n60 dumpviews\n70 waittext Welcome back\n80 clicktext OK\n")
        .unwrap();
    let k = |key, pressed| Action::Event(Event::Key { key: Key(key), pressed, repeat: false, mods: Mods::default() });
    assert_eq!(s[0], (0, k(40, true)));
    assert_eq!(s[1], (10, k(80, true)));
    assert_eq!(s[2], (20, k(80, false)));
    assert_eq!(s[3], (30, Action::Event(Event::Text("hello  world".into()))));
    assert_eq!(s[4], (40, k(40, false)));
    assert_eq!(s[5], (40, Action::Quit));
    assert_eq!(s[6], (50, Action::Mark("in_world".into())));
    assert_eq!(s[7], (60, Action::DumpViews));
    assert_eq!(s[8], (70, Action::WaitText("Welcome back".into())));
    assert_eq!(s[9], (80, Action::ClickText("OK".into())));
    assert_eq!(parse("0 waittext\n").unwrap_err(), "line 1: waittext needs a text");
    assert_eq!(click_events(1.0, 2.0).len(), 3);
    for (n, want) in [
        ("a", 4),
        ("Z", 29),
        ("1", 30),
        ("9", 38),
        ("0", 39),
        ("ESC", 41),
        ("backspace", 42),
        ("tab", 43),
        ("space", 44),
        ("delete", 76),
        ("right", 79),
        ("down", 81),
        ("up", 82),
        ("return", 40),
        ("escape", 41),
    ] {
        assert_eq!(key_code(n), Some(Key(want)), "{n}");
    }
    assert_eq!(key_code("f13"), None);
    assert_eq!(key_code("?"), None);
}

#[test]
fn errors_name_the_line() {
    for (text, want) in [
        ("click 1 2", "line 1: bad time"),
        ("0\n", "line 1: missing command"),
        ("# x\n0 click 1\n", "line 2: missing or bad y"),
        ("0 key f13\n", "unknown key"),
        ("0 text\n", "text needs a string"),
        ("0 shot a/b\n", "shot needs a plain name"),
        ("0 shot\n", "shot needs a plain name"),
        ("0 mark\n", "mark needs a label"),
        ("0 dance\n", "unknown command"),
        ("+x click 1 2\n", "bad time"),
        ("0 drag 1 2 3 4\n", "missing or bad ms"),
    ] {
        let e = parse(text).unwrap_err();
        assert!(e.contains(want), "{text:?} -> {e}");
    }
}

#[test]
fn build_merges_file_and_click_shortcuts() {
    let s = build(Some("500 key a\n"), &[(1.0, 2.0, 100)]).unwrap();
    assert_eq!(s.len(), 5);
    assert_eq!(s[0].0, 100);
    assert_eq!(click_line(1.0, 2.5, 7), "7 click 1 2.5\n");
    assert!(build(None, &[]).unwrap().is_empty());
    assert!(build(Some("bogus"), &[]).is_err());
}

#[test]
fn window_commands() {
    use bhpc_app::Event;
    let s = parse("0 focus off\n+10 focus on\n+10 minimize\n+10 restore\n+10 resize 1600 900\n+10 key f11\n").unwrap();
    assert_eq!(s[0].1, Action::Event(Event::Focus(false)));
    assert_eq!(s[1].1, Action::Event(Event::Focus(true)));
    assert_eq!(s[2].1, Action::Event(Event::Visible(false)));
    assert_eq!(s[3].1, Action::Event(Event::Visible(true)));
    assert_eq!(s[4].1, Action::Resize(1600, 900));
    assert!(matches!(&s[5].1, Action::Event(Event::Key { key, pressed: true, .. }) if key.0 == 68));
    assert!(parse("0 focus maybe").unwrap_err().contains("line 1"));
    assert!(parse("0 resize 5").is_err());
}
