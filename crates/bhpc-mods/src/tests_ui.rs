use std::time::Instant;

use crate::host::{LogLine, ModStatus, Usage};
use crate::ui::*;
use crate::view::{JoinMod, JoinStage, JoinView, UiCmd, View};

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
    }
}

fn prompt(mods: Vec<JoinMod>) -> View {
    View {
        join: Some(JoinView {
            server: "Sunny Isle".into(),
            stage: JoinStage::Prompt,
            mods,
            required: true,
            progress: (0, 778_240),
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
    // F9 does nothing while the prompt shows; clicks outside buttons are taken.
    assert_eq!(ui.input(UiInput::Key(HID_F9), &v), (true, None));
    assert_eq!(ui.input(UiInput::Click(1.0, 1.0), &v), (true, None));
    // Click the primary button: it is the right-most hit area at the bottom of the dialog.
    let mut found = None;
    for y in (300..720).step_by(4) {
        for x in (640..1280).rev().step_by(8) {
            if let (true, Some(cmd)) = ui.input(UiInput::Click(x as f32, y as f32), &v) {
                found = Some(cmd);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    assert_eq!(found, Some(UiCmd::Decide { accept: true, native_ok: false }));
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
    let mut toggled = false;
    for y in (0..720).step_by(3) {
        if ui.input(UiInput::Click(500.0, y as f32), &v) == (true, None) && !ui.native_ok {
            toggled = true;
            break;
        }
    }
    assert!(toggled);
    // A refused native mod: no checkbox, cannot accept.
    if let Some(j) = v.join.as_mut() {
        j.mods[0].refused = true;
        j.mods[0].cached = true;
        j.mods.push(jm("Solo", false, &[]));
        j.mods[2].name = "1".into();
    }
    render(&mut ui, &v, 2, "native-refused");
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
    let mut v = prompt(vec![a, b, c]);
    let j = v.join.as_mut().unwrap();
    j.stage = JoinStage::Downloading;
    j.progress = (519_120, 1_167_360);
    render(&mut ui, &v, 1, "downloading");
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, Some(UiCmd::Cancel)));
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, None));
    let j = v.join.as_mut().unwrap();
    j.progress = (0, 0);
    render(&mut ui, &v, 9, "downloading-empty");
    for (i, (stage, name)) in [
        (JoinStage::Failed("coin-bank: expected sha256 41aa, got 77d1".into()), "check-failed"),
        (JoinStage::Failed("coin-bank: the download stopped at 130 of 210 bytes".into()), "download-failed"),
        (
            JoinStage::Rejected("You need the mod loader 0.2".into(), Some("https://blockheads.club/mods".into())),
            "rejected",
        ),
        (JoinStage::Rejected("Stale mods".into(), None), "rejected-plain"),
    ]
    .into_iter()
    .enumerate()
    {
        let v = View { join: Some(JoinView { stage, ..v.join.clone().unwrap() }), ..View::default() };
        render(&mut ui, &v, 20 + i as u64, name);
        assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, Some(UiCmd::Dismiss)));
        assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (true, Some(UiCmd::Dismiss)));
        assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, None));
    }
}

fn status(id: &str, state: &'static str) -> ModStatus {
    ModStatus { id: id.into(), version: "1.0.0".into(), state, reason: None, usage: Usage::default() }
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
