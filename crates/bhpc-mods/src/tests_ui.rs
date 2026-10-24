use std::time::Instant;

use std::time::Duration;

use bhmod_core::timings::{HandlerKind, KindStats, ModTimings};

use crate::host::{LogLine, ModStatus, Usage};
use crate::ui::*;
use crate::view::{Fail, FailKind, JoinMod, JoinStage, JoinView, UiCmd, View};

fn jm(name: &str, native: bool, perms: &[&str]) -> JoinMod {
    JoinMod {
        name: name.into(),
        version: "1.2.0".into(),
        size: 389_120,
        have: 0,
        cached: false,
        done: false,
        native,
        refused: false,
        required: true,
        enabled: true,
        permissions: perms.iter().map(|s| s.to_string()).collect(),
        listed: !native,
        with_server: true,
    }
}

fn prompt(mods: Vec<JoinMod>) -> View {
    View {
        join: Some(JoinView {
            server: "Sunny Isle".into(),
            address: "mira.example.net:15151".into(),
            stage: JoinStage::Prompt,
            mods,
            required: true,
            progress: (0, 778_240),
            eta: None,
            players: None,
            retry: false,
        }),
        ..View::default()
    }
}

/// `BHPC_MODS_UI_DUMP=DIR` writes each picture as `<name>.pam` (convert with ImageMagick for a look).
fn dump(name: &str, w: u32, h: u32, px: &[u8]) {
    if let Some(dir) = std::env::var_os("BHPC_MODS_UI_DUMP") {
        let mut out =
            format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();
        out.extend_from_slice(px);
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.pam")), out).unwrap();
    }
}

fn render(ui: &mut ModUi, v: &View, serial: u64, name: &str) -> std::sync::Arc<Vec<u8>> {
    let px = ui.render(v, serial, 1280, 720).unwrap();
    dump(name, 1280, 720, &px);
    assert!(px.chunks(4).any(|p| p[3] == 255));
    px
}

/// Clicks the centre of the first hit area of `hit` (the picture must have been rendered).
fn click(ui: &mut ModUi, v: &View, hit: Hit) -> (bool, Option<UiCmd>) {
    let r = ui.hits_for_tests().into_iter().find(|(_, h)| *h == hit).unwrap_or_else(|| panic!("no {hit:?}")).0;
    ui.input(UiInput::Click((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32), v)
}

fn optional(mut m: JoinMod) -> JoinMod {
    m.required = false;
    m
}

#[test]
fn join_prompt_keys_and_clicks() {
    let mut ui = ModUi::new();
    let v = prompt(vec![jm("Land Claims", false, &["world:read", "channels"]), jm("Coin Bank", false, &[])]);
    assert!(ui.modal(&v));
    let a = render(&mut ui, &v, 1, "prompt");
    // The same view gives the same picture without drawing.
    assert!(std::sync::Arc::ptr_eq(&a, &ui.render(&v, 1, 1280, 720).unwrap()));
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, Some(UiCmd::Decide { accept: true, native_ok: false })));
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, Some(UiCmd::Decide { accept: false, native_ok: false })));
    assert_eq!(ui.input(UiInput::Key(99), &v), (true, None));
    assert_eq!(ui.input(UiInput::Other, &v), (true, None));
    // Nothing to step through when every mod is required.
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, None));
    assert_eq!(ui.input(UiInput::Key(HID_DOWN), &v), (true, None));
    // F9 does nothing while the prompt shows; clicks outside buttons are taken.
    assert_eq!(ui.input(UiInput::Key(HID_F9), &v), (true, None));
    assert_eq!(ui.input(UiInput::Click(1.0, 1.0), &v), (true, None));
    // A server known by its address (no letters) gets a question mark on its tile.
    let mut by_address = v.clone();
    by_address.join.as_mut().unwrap().server = "127.0.0.1:15151".into();
    render(&mut ui, &by_address, 5, "prompt-address");
    render(&mut ui, &v, 6, "prompt");
    assert_eq!(click(&mut ui, &v, Hit::Accept), (true, Some(UiCmd::Decide { accept: true, native_ok: false })));
    assert_eq!(click(&mut ui, &v, Hit::Decline), (true, Some(UiCmd::Decide { accept: false, native_ok: false })));
}

#[test]
fn optional_mods_have_switches_and_focus() {
    let mut ui = ModUi::new();
    let mut off = optional(jm("Shop Signs", false, &["net-http"]));
    off.enabled = false;
    let mut cached = optional(jm("Coin Bank", false, &["world:write", "players:chat", "other"]));
    cached.cached = true;
    cached.with_server = false;
    let mut unreviewed = optional(jm("Unreviewed", false, &[]));
    unreviewed.listed = false;
    let v = prompt(vec![
        jm("Land Claims", false, &["world:read"]),
        optional(jm("Minimap", false, &[])),
        off,
        cached,
        unreviewed,
    ]);
    render(&mut ui, &v, 1, "prompt-optional");
    // Focus starts on the first switch; Down/Up wrap around the optional mods.
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, Some(UiCmd::Toggle(1))));
    ui.input(UiInput::Key(HID_DOWN), &v);
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, Some(UiCmd::Toggle(2))));
    ui.input(UiInput::Key(HID_DOWN), &v);
    ui.input(UiInput::Key(HID_DOWN), &v);
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, Some(UiCmd::Toggle(4))));
    ui.input(UiInput::Key(HID_DOWN), &v);
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, Some(UiCmd::Toggle(1))), "wraps around");
    ui.input(UiInput::Key(HID_UP), &v);
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, Some(UiCmd::Toggle(4))));
    ui.input(UiInput::Key(HID_UP), &v);
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, Some(UiCmd::Toggle(3))));
    // The focus ring changes the picture; clicking a row toggles it, the required one has no switch.
    let moved = render(&mut ui, &v, 1, "prompt-optional-focus");
    assert!(!std::sync::Arc::ptr_eq(&moved, &ui.render(&v, 2, 1280, 720).unwrap()));
    assert_eq!(click(&mut ui, &v, Hit::Toggle(2)), (true, Some(UiCmd::Toggle(2))));
    assert!(ui.hits_for_tests().iter().all(|(_, h)| *h != Hit::Toggle(0)));
    // A refused optional mod cannot be switched.
    let mut refused = v.clone();
    let j = refused.join.as_mut().unwrap();
    j.mods[1].refused = true;
    j.mods[1].enabled = false;
    render(&mut ui, &refused, 3, "prompt-optional-refused");
    assert!(ui.hits_for_tests().iter().all(|(_, h)| *h != Hit::Toggle(1)));
    // Nothing to download: the tag says so.
    let mut none = v.clone();
    for m in &mut none.join.as_mut().unwrap().mods {
        m.cached = true;
    }
    render(&mut ui, &none, 4, "prompt-cached");
}

#[test]
fn native_prompt_needs_the_box() {
    let mut ui = ModUi::new();
    let mut v = prompt(vec![
        jm("Turbo Physics", true, &[]),
        jm("Land Claims", false, &["players:chat", "world:write", "net-http", "other"]),
    ]);
    render(&mut ui, &v, 1, "native");
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, None));
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, None));
    assert!(ui.native_ok);
    render(&mut ui, &v, 1, "native-ok");
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, Some(UiCmd::Decide { accept: true, native_ok: true })));
    // The box toggles by click too.
    assert_eq!(click(&mut ui, &v, Hit::NativeBox), (true, None));
    assert!(!ui.native_ok);
    // A refused native mod: no checkbox, cannot accept.
    if let Some(j) = v.join.as_mut() {
        j.mods[0].refused = true;
        j.mods[0].cached = true;
        j.mods.push(jm("Solo", false, &[]));
        j.mods[2].name = "1".into();
    }
    render(&mut ui, &v, 2, "native-refused");
    assert!(ui.hits_for_tests().iter().all(|(_, h)| *h != Hit::NativeBox));
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, None));
    // An optional native mod that is switched off asks for nothing.
    let mut off = prompt(vec![jm("Req", false, &[]), optional(jm("Turbo", true, &[]))]);
    off.join.as_mut().unwrap().mods[1].enabled = false;
    render(&mut ui, &off, 3, "native-off");
    assert!(ui.hits_for_tests().iter().all(|(_, h)| *h != Hit::NativeBox));
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &off), (true, Some(UiCmd::Decide { accept: true, native_ok: false })));
}

#[test]
fn downloading_failed_and_rejected() {
    let mut ui = ModUi::new();
    let mut a = jm("Land Claims", false, &[]);
    a.done = true;
    a.have = a.size;
    let mut b = jm("Coin Bank", false, &[]);
    b.have = 130_000;
    let c = jm("Shop Signs", false, &[]);
    let mut off = optional(jm("Skipped", false, &[]));
    off.enabled = false;
    let mut v = prompt(vec![a, b, c, off]);
    let j = v.join.as_mut().unwrap();
    j.stage = JoinStage::Downloading;
    j.progress = (519_120, 1_167_360);
    j.eta = Some(4);
    render(&mut ui, &v, 1, "downloading");
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, Some(UiCmd::Cancel)));
    assert_eq!(click(&mut ui, &v, Hit::Cancel), (true, Some(UiCmd::Cancel)));
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, None));
    let j = v.join.as_mut().unwrap();
    j.progress = (0, 0);
    j.eta = None;
    render(&mut ui, &v, 9, "downloading-empty");
    let check = FailKind::Check {
        name: "Coin Bank".into(),
        version: "1.2.0".into(),
        expected: "77ae".repeat(16),
        got: Some("c013".repeat(16)),
    };
    let fail = |kind, text: &str| JoinStage::Failed(Fail { kind, text: text.into() });
    for (i, (stage, name)) in [
        (fail(check, "It may be broken."), "check-failed"),
        (
            fail(FailKind::Download { name: "Coin Bank".into(), percent: 40 }, "The connection was lost."),
            "download-failed",
        ),
        (fail(FailKind::Newer, "Sunny Isle needs a newer mod loader."), "needs-newer"),
        (fail(FailKind::Other, "the mods did not start: boom"), "failed-other"),
        (
            JoinStage::Rejected("Update your mod loader to 0.2".into(), Some("https://blockheads.club/mods".into())),
            "rejected-newer",
        ),
        (JoinStage::Rejected("Stale mods".into(), Some("https://blockheads.club/mods".into())), "rejected"),
        (JoinStage::Rejected("Stale mods".into(), None), "rejected-plain"),
    ]
    .into_iter()
    .enumerate()
    {
        let copy = matches!(&stage, JoinStage::Failed(f) if matches!(f.kind, FailKind::Check { .. }));
        let v = View { join: Some(JoinView { stage, ..v.join.clone().unwrap() }), ..View::default() };
        render(&mut ui, &v, 20 + i as u64, name);
        assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, Some(UiCmd::Dismiss)));
        // The newer-loader notice's main button is "Update after this game".
        let newer = name == "needs-newer";
        let enter = if newer { UiCmd::UpdateLater } else { UiCmd::Dismiss };
        assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, Some(enter)));
        assert_eq!(click(&mut ui, &v, Hit::Dismiss), (true, Some(UiCmd::Dismiss)));
        assert_eq!(ui.hits_for_tests().iter().any(|(_, h)| *h == Hit::UpdateLater), newer);
        if newer {
            assert_eq!(click(&mut ui, &v, Hit::UpdateLater), (true, Some(UiCmd::UpdateLater)));
        }
        assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, None));
        // "Copy details" only exists for a failed file check.
        let key = ui.input(UiInput::Key(HID_Y), &v);
        assert_eq!(matches!(key.1, Some(UiCmd::Copy(_))), copy);
        assert_eq!(ui.hits_for_tests().iter().any(|(_, h)| *h == Hit::Copy), copy);
        if copy {
            let (_, Some(UiCmd::Copy(text))) = click(&mut ui, &v, Hit::Copy) else { panic!() };
            assert!(text.contains("expected sha256 77ae77ae") && text.contains("Coin Bank 1.2.0"), "{text}");
            assert!(text.contains("got sha256 c013c013"), "{text}");
        }
    }
    // A short hash is shown whole, and a stale Copy hit on another view does nothing.
    let short = FailKind::Check { name: "A".into(), version: "1".into(), expected: "ab".into(), got: None };
    let v = View { join: Some(JoinView { stage: fail(short, "x"), ..v.join.clone().unwrap() }), ..View::default() };
    render(&mut ui, &v, 40, "check-failed-short");
    let r = ui.hits_for_tests().into_iter().find(|(_, h)| *h == Hit::Copy).unwrap().0;
    let plain = prompt(vec![jm("A", false, &[])]);
    assert_eq!(ui.input(UiInput::Click((r.x + 2) as f32, (r.y + 2) as f32), &plain), (true, None));
    // Details of the other failures, and Copy outside a failure does nothing.
    let f = |kind| Fail { kind, text: "t".into() };
    assert_eq!(crate::ui::details_for_tests(&f(FailKind::Other)), "t");
    assert!(crate::ui::details_for_tests(&f(FailKind::Download { name: "n".into(), percent: 3 }))
        .starts_with("n stopped at 3 %"));
    assert_eq!(ui.input(UiInput::Key(HID_Y), &plain), (true, None));
}

#[test]
fn a_crowded_prompt_is_cut_to_the_window() {
    let mut ui = ModUi::new();
    let mods: Vec<JoinMod> =
        (0..14).map(|i| jm(&format!("Mod {i}"), i == 3, &["channels", "world:read", "net-http"])).collect();
    let v = prompt(mods);
    let px = ui.render(&v, 1, 800, 450).unwrap();
    dump("prompt-crowded", 800, 450, &px);
    // A phone-sized window still draws (the scale has a floor).
    let px = ui.render(&v, 2, 320, 200).unwrap();
    dump("prompt-tiny", 320, 200, &px);
}

/// The time accounting of a mod: the kinds are `(kind, calls, total ms, max ms)`.
pub(crate) fn timings(id: &str, kinds: &[(HandlerKind, u64, u64, u64)]) -> ModTimings {
    ModTimings {
        id: id.into(),
        state: "enabled".into(),
        window: Duration::from_secs(60),
        kinds: kinds
            .iter()
            .map(|&(kind, calls, total, max)| KindStats {
                kind: Some(kind),
                calls,
                total: Duration::from_millis(total),
                max: Duration::from_millis(max),
                soft_hits: 1,
                timeouts: 0,
                errors: 0,
            })
            .collect(),
        strikes: 0,
        strikes_total: 0,
    }
}

fn status(id: &str, state: &'static str) -> ModStatus {
    ModStatus {
        id: id.into(),
        version: "1.0.0".into(),
        state,
        reason: None,
        usage: Usage::default(),
        timings: timings(id, &[]),
    }
}

#[test]
fn mods_panel_and_toasts() {
    let mut ui = ModUi::new();
    let mut v = View::default();
    assert!(!ui.modal(&v));
    assert_eq!(ui.render(&v, 0, 1280, 720), None);
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (false, None));
    assert_eq!(ui.input(UiInput::Key(HID_F5), &v), (false, None));
    let dev = View { dev: true, ..View::default() };
    assert_eq!(ui.input(UiInput::Key(HID_F5), &dev), (true, Some(UiCmd::Reload)));
    assert_eq!(ui.input(UiInput::Key(HID_F9), &v), (true, None));
    assert!(ui.panel && ui.modal(&v));
    render(&mut ui, &v, 1, "panel-empty");
    v.server = Some("Sunny Isle".into());
    v.mods = vec![
        (status("echo", "running"), "server"),
        (status("minimap", "budget"), "mine"),
        (status("cozy", "failed"), "dev"),
        (status("zz", "loaded"), "mine"),
    ];
    let long = "x".repeat(300);
    v.log = ["info", "warn", "error"]
        .iter()
        .map(|l| LogLine { ms: 1, level: l, mod_id: Some("echo".into()), text: format!("hello {l} {long}") })
        .chain(std::iter::once(LogLine { ms: 2, level: "info", mod_id: None, text: "host".into() }))
        .collect();
    v.toasts = vec![(Instant::now(), "Zoom Out was turned off. It slowed the game.".into())];
    render(&mut ui, &v, 2, "panel");
    v.dev = true;
    render(&mut ui, &v, 5, "panel-dev");
    v.dev = false;
    assert_eq!(ui.input(UiInput::Key(99), &v), (true, None));
    assert_eq!(ui.input(UiInput::Click(0.0, 0.0), &v), (true, None));
    // The close button sits in the banner, top right.
    let mut closed = false;
    'outer: for y in (0..80).step_by(2) {
        for x in (1100..1280).step_by(2) {
            ui.input(UiInput::Click(x as f32, y as f32), &v);
            if !ui.panel {
                closed = true;
                break 'outer;
            }
        }
    }
    assert!(closed);
    ui.input(UiInput::Key(HID_F9), &v);
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, None));
    assert!(!ui.panel);
    assert_eq!(ui.input(UiInput::Click(0.0, 0.0), &v), (false, None));
    // So many mods that no room is left for the log.
    let mut full = v.clone();
    full.mods = (0..12).map(|i| (status(&format!("m{i}"), "running"), "mine")).collect();
    ui.input(UiInput::Key(HID_F9), &full);
    render(&mut ui, &full, 4, "panel-full");
    ui.input(UiInput::Key(HID_F9), &full);
    // Only toasts: not modal, still drawn (small window: u stays at 2).
    let px = ui.render(&v, 3, 640, 360).unwrap();
    dump("toast-small", 640, 360, &px);
    assert!(!ui.modal(&v));
}

#[test]
fn the_panel_shows_the_time_accounting() {
    use crate::ui::{HID_R, HID_T};
    let mut ui = ModUi::new();
    let mut v = View::default();
    // Without the panel the keys belong to the game.
    assert_eq!(ui.input(UiInput::Key(HID_T), &v), (false, None));
    ui.input(UiInput::Key(HID_F9), &v);
    // R only resets while the times are shown.
    assert_eq!(ui.input(UiInput::Key(HID_R), &v), (true, None));
    render(&mut ui, &v, 1, "panel-log-empty");
    v.mods = vec![(status("echo", "running"), "server"), (status("busy", "running"), "mine")];
    v.mods[1].0.timings = timings(
        "busy",
        &[(HandlerKind::Event, 120, 30, 4), (HandlerKind::Command, 2, 9, 7), (HandlerKind::Task, 40, 2, 1)],
    );
    v.mods[1].0.timings.strikes_total = 2;
    v.mods[1].0.timings.strikes = 1;
    let log_picture = render(&mut ui, &v, 2, "panel-log");
    // T (or the button) switches to the times, and back.
    assert_eq!(ui.input(UiInput::Key(HID_T), &v), (true, None));
    assert!(ui.timings);
    let times_picture = render(&mut ui, &v, 2, "panel-times");
    assert_ne!(log_picture, times_picture);
    assert!(ui.hits_for_tests().iter().any(|(_, h)| *h == Hit::ResetTimings));
    assert_eq!(ui.input(UiInput::Key(HID_R), &v), (true, Some(UiCmd::ResetTimings)));
    assert_eq!(click(&mut ui, &v, Hit::ResetTimings), (true, Some(UiCmd::ResetTimings)));
    assert_eq!(click(&mut ui, &v, Hit::ToggleTimings), (true, None));
    assert!(!ui.timings);
    render(&mut ui, &v, 2, "panel-log-again");
    assert!(ui.hits_for_tests().iter().all(|(_, h)| *h != Hit::ResetTimings));
    // The developer's hint, and no mods at all in the time view.
    v.dev = true;
    render(&mut ui, &v, 3, "panel-log-dev");
    ui.input(UiInput::Key(HID_T), &v);
    v.mods.clear();
    render(&mut ui, &v, 4, "panel-times-empty");
}

#[test]
fn text_helpers() {
    assert_eq!(size_text(10), "10 B");
    assert_eq!(size_text(2048), "2 KB");
    assert_eq!(size_text(3 << 20), "3.0 MB");
    let lines = wrap(Face::Body, 16.0, "one two three four five six seven", 60.0);
    assert!(lines.len() > 2);
    assert_eq!(wrap(Face::Body, 16.0, "a\nb", 600.0), ["a", "b"]);
    assert!(measure(Face::Pixel, 24.0, "AV") > 0.0);
    let mut c = Canvas::new(4, 4);
    c.fill(-2, -2, 10, 10, 0xffffff, 0.0);
    assert!(c.px.iter().all(|b| *b == 0));
}

#[test]
fn try_again_and_the_player_count() {
    let mut ui = ModUi::new();
    let base = prompt(vec![jm("Coin Bank", true, &["channels"])]);
    let down = JoinStage::Failed(Fail {
        kind: FailKind::Download { name: "Coin Bank".into(), percent: 40 },
        text: "The connection was lost.".into(),
    });
    let with = |stage: JoinStage, retry: bool, players: Option<(u32, u32)>| View {
        join: Some(JoinView { stage, retry, players, ..base.join.clone().unwrap() }),
        ..View::default()
    };
    // A download that stopped, on a flow that can start again: Enter is "Try again", Esc leaves.
    let v = with(down.clone(), true, Some((3, 16)));
    let shown = render(&mut ui, &v, 1, "download-failed-retry");
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, Some(UiCmd::Retry)));
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, Some(UiCmd::Dismiss)));
    assert_eq!(click(&mut ui, &v, Hit::Retry), (true, Some(UiCmd::Retry)));
    assert_eq!(click(&mut ui, &v, Hit::Dismiss), (true, Some(UiCmd::Dismiss)));
    // Without the retry path (or for another failure) there is no such button.
    let v2 = with(down, false, Some((3, 16)));
    render(&mut ui, &v2, 4, "download-failed");
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v2), (true, Some(UiCmd::Dismiss)));
    assert!(!ui.hits_for_tests().iter().any(|(_, h)| *h == Hit::Retry));
    let other = JoinStage::Failed(Fail { kind: FailKind::Other, text: "x".into() });
    let v3 = with(other, true, None);
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v3), (true, Some(UiCmd::Dismiss)));
    // The count appears in the address line only when there is one.
    let no_count = render(&mut ui, &with(JoinStage::Prompt, false, None), 2, "prompt-no-count");
    let count = render(&mut ui, &with(JoinStage::Prompt, false, Some((3, 16))), 3, "prompt-count");
    assert_ne!(no_count, count);
    assert_ne!(shown, count);
}

#[test]
fn body_text_falls_back_to_liberation_for_glyphs_atkinson_lacks() {
    let latin = measure(Face::Body, 20.0, "Mod");
    let cyr = measure(Face::Body, 20.0, "Мод");
    assert!(latin > 0.0 && cyr > 0.0);
    // Mixed text measures and draws (the kerning pair across two fonts is skipped).
    let mut c = Canvas::new(200, 40);
    let w = c.text(Face::Body, 20.0, 2.0, 2.0, 0xFFFFFF, "aМ a");
    assert!(w > 0.0 && c.px.iter().any(|b| *b != 0));
    assert_eq!(w, measure(Face::Body, 20.0, "aМ a"));
}
