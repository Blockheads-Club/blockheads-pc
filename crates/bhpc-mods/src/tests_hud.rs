//! The mods' HUD on PC: widgets, toasts, notices, dialogs (model, bridge, drawing, input).

use std::io::Write;
use std::time::{Duration, Instant};

use bhmod_core::ui::{ButtonKind, Dialog, DialogButton, Field, NoticeLevel, Slot, Widget, WidgetKind, CLOSED};

use crate::host::*;
use crate::hud::*;
use crate::join::Accepted;
use crate::session::{build, Plan};
use crate::ui::*;
use crate::view::{UiCmd, View};

fn widget(slot: Slot, kind: WidgetKind, title: Option<&str>, text: &str) -> Widget {
    Widget { slot, kind, icon: None, title: title.map(str::to_string), text: text.into() }
}

fn dialog(field: bool, buttons: usize) -> Dialog {
    Dialog {
        title: "Volume of Ann".into(),
        text: "How loud should Ann be?".into(),
        field: field.then(|| Field { placeholder: "0-200".into(), initial: "10".into(), max_length: 3 }),
        buttons: (0..buttons)
            .map(|i| DialogButton {
                label: format!("B{i}"),
                kind: [ButtonKind::Primary, ButtonKind::Secondary, ButtonKind::Danger][i % 3],
            })
            .collect(),
    }
}

#[test]
fn the_hud_model() {
    let d = tempfile::tempdir().unwrap();
    let file = Hud::hidden_path(d.path());
    let mut h = Hud::new(Some(file.clone()));
    let t = Instant::now();
    h.set_widget("a", "w1", widget(Slot::TopLeft, WidgetKind::Text, None, "x"));
    h.set_widget("a", "w1", widget(Slot::TopLeft, WidgetKind::Text, None, "y"));
    h.set_widget("b", "w2", widget(Slot::TopRight, WidgetKind::Counter, Some("Coins"), "7"));
    assert_eq!(h.widgets().len(), 2);
    assert_eq!(h.widgets()[0].widget.text, "y");
    assert!(h.has_widgets("a") && !h.has_widgets("c"));
    h.set_hidden("a", true).unwrap();
    assert!(h.is_hidden("a"));
    assert_eq!(h.widgets().len(), 1);
    assert!(Hud::new(Some(file.clone())).is_hidden("a"), "kept on the device");
    h.set_hidden("a", false).unwrap();
    assert_eq!(h.widgets().len(), 2);
    h.remove_widget("b", "w2");
    assert_eq!(h.widgets().len(), 1);
    h.toast("hi", t);
    for i in 0..5 {
        h.notice(NoticeLevel::Info, &format!("n{i}"), t);
    }
    assert_eq!(h.notices().len(), MAX_NOTICES);
    assert_eq!(h.notices()[0].text, "n2");
    assert_eq!(h.toasts().len(), 1);
    assert!(!h.expire(t + Duration::from_secs(1)));
    assert!(h.expire(t + TOAST_TIME));
    assert!(h.toasts().is_empty() && !h.notices().is_empty());
    assert!(h.expire(t + NOTICE_TIME));
    h.show_dialog("a", 1, dialog(false, 1));
    h.show_dialog("b", 2, dialog(false, 1));
    assert_eq!(h.dialog().unwrap().id, 1);
    assert!(h.answered(1));
    assert!(!h.answered(1));
    assert!(!h.close_dialog(9));
    h.clear_mod("b");
    assert!(h.dialog().is_none());
    h.show_dialog("a", 3, dialog(false, 1));
    let s = h.serial;
    assert_eq!(h.clear(), vec![3]);
    assert!(h.serial > s && h.widgets().is_empty());
    // Without a file the choice holds for the session only; an unreadable file means nothing hidden.
    let mut mem = Hud::new(None);
    mem.set_hidden("a", true).unwrap();
    assert!(Hud::new(Some(d.path().join("none"))).widgets().is_empty());
}

const MAIN: &str = r#"
bh.events.on_custom("hm:ui-action", function(ev)
    local v = ev.payload
    bh.log.info("answer " .. v.button .. " " .. tostring(v.text))
end)
return {
    on_enable = function()
        bh.ui.set_widget("coins", { slot = "top-right", kind = "counter", title = "Coins", text = "7" })
        bh.ui.set_widget("gone", { text = "x" })
        bh.ui.remove_widget("gone")
        bh.ui.toast("hello")
        bh.ui.notice("careful", "warning")
        local d2 = bh.ui.show_dialog({ title = "Other", buttons = { { label = "OK" } } })
        bh.ui.close_dialog(d2)
        local d = bh.ui.show_dialog({ title = "Name", text = "Who?", field = { initial = "ab" },
            buttons = { { label = "OK", kind = "primary" } } })
        bh.log.info("dialog " .. d)
    end,
}
"#;

fn package() -> Vec<u8> {
    let toml = "schema = 1\nid = \"hm\"\nversion = \"1.0.0\"\nname = { en = \"HUD\" }\nauthors = [\"a\"]\n\
                license = \"MIT\"\napi = \"^0.3.4\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\n\
                path = \"client/main.luau\"\n";
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("mod.toml", o).unwrap();
    w.write_all(toml.as_bytes()).unwrap();
    w.start_file("client/main.luau", o).unwrap();
    w.write_all(MAIN.as_bytes()).unwrap();
    w.finish().unwrap().into_inner()
}

#[test]
fn a_mod_drives_the_hud_through_the_bridge() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("hm.bhmod");
    std::fs::write(&p, package()).unwrap();
    let plan = Plan { own: None, dev: vec![], server: vec![Accepted { id: "hm".into(), path: p, grant: vec![] }] };
    let mods = d.path().join("session");
    build(&mods, &plan).unwrap();
    let mut h = ClientHost::load(HostSetup {
        mods_dir: &mods,
        storage_dir: &d.path().join("storage"),
        runtimes: bhmod_core::default_runtimes(),
        online: true,
        local_player: "ME".into(),
        voice: None,
        binds: None,
        client_settings: None,
        hud: Some(Hud::hidden_path(d.path())),
    });
    h.enable("W", "w");
    let logs: Vec<String> = h.take_new_logs().into_iter().map(|l| l.text).collect();
    let id: u32 =
        logs.iter().find_map(|l| l.strip_prefix("dialog ")).unwrap_or_else(|| panic!("{logs:?}")).parse().unwrap();
    {
        let hud = h.hud();
        assert_eq!(hud.widgets().len(), 1);
        assert_eq!(hud.toasts()[0].1, "hello");
        assert_eq!((hud.notices()[0].level, hud.notices()[0].text.as_str()), (NoticeLevel::Warning, "careful"));
        assert_eq!(hud.dialog().unwrap().id, id, "the closed one is gone");
    }
    h.notice(NoticeLevel::Error, "loader");
    assert_eq!(h.hud().notices().len(), 2);
    assert!(!h.expire_hud(Instant::now()));
    h.hide_widgets("hm", true).unwrap();
    assert!(h.hud().widgets().is_empty());
    assert!(h.dialog_answer(id, 0, Some("abc".into())));
    assert!(!h.dialog_answer(id, 0, None));
    let logs: Vec<String> = h.take_new_logs().into_iter().map(|l| l.text).collect();
    assert!(logs.contains(&"answer 0 abc".to_string()), "{logs:?}");
    h.stop();
    assert!(h.hud().notices().is_empty());
}

fn view_with(d: Option<Dialog>) -> View {
    let t = Instant::now();
    View {
        widgets: vec![
            ModWidget {
                mod_id: "a".into(),
                id: "1".into(),
                widget: widget(Slot::TopLeft, WidgetKind::Text, Some("T"), "hello"),
            },
            ModWidget {
                mod_id: "a".into(),
                id: "2".into(),
                widget: widget(Slot::TopLeft, WidgetKind::Panel, None, &"long words here ".repeat(30)),
            },
            ModWidget {
                mod_id: "b".into(),
                id: "3".into(),
                widget: widget(Slot::TopRight, WidgetKind::Counter, Some("Coins"), "7"),
            },
        ],
        notices: [NoticeLevel::Info, NoticeLevel::Warning, NoticeLevel::Success, NoticeLevel::Error]
            .into_iter()
            .map(|level| Notice { at: t, level, text: "This server has Voice (voice chat): update".into() })
            .collect(),
        dialog: d.map(|dialog| ModDialog { mod_id: "a".into(), id: 5, dialog }),
        ..View::default()
    }
}

#[test]
fn widgets_and_notices_are_drawn_without_taking_input() {
    let v = view_with(None);
    let mut ui = ModUi::new();
    assert!(!ui.modal(&v));
    assert!(ui.render(&v, 1, 1280, 720).is_some());
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v), (false, None));
    // A tiny window: widgets that do not fit are left out.
    assert!(ui.render(&v, 2, 200, 60).is_some());
}

fn click(ui: &mut ModUi, v: &View, hit: Hit) -> (bool, Option<UiCmd>) {
    let r = ui.hits_for_tests().into_iter().find(|(_, h)| *h == hit).unwrap_or_else(|| panic!("no {hit:?}")).0;
    ui.input(UiInput::Click((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32), v)
}

#[test]
fn a_dialog_with_a_field() {
    let v = view_with(Some(dialog(true, 3)));
    let mut ui = ModUi::new();
    assert!(ui.modal(&v));
    ui.render(&v, 1, 1280, 720).unwrap();
    assert_eq!(ui.dialog_text, "10");
    ui.input(UiInput::Char('0'), &v);
    ui.input(UiInput::Char('9'), &v);
    ui.input(UiInput::Char('\n'), &v);
    assert_eq!(ui.dialog_text, "100", "at most 3 characters, no control characters");
    ui.input(UiInput::Key(HID_BACKSPACE), &v);
    ui.render(&v, 1, 1280, 720).unwrap();
    assert_eq!(
        ui.input(UiInput::Key(HID_ENTER), &v),
        (true, Some(UiCmd::DialogAnswer { id: 5, button: 0, text: Some("10".into()) }))
    );
    // Opened again: the initial text; an emptied field shows the placeholder.
    for _ in 0..2 {
        ui.input(UiInput::Key(HID_BACKSPACE), &v);
    }
    ui.render(&v, 2, 1280, 720).unwrap();
    assert_eq!(
        click(&mut ui, &v, Hit::DialogButton(2)).1,
        Some(UiCmd::DialogAnswer { id: 5, button: 2, text: Some(String::new()) })
    );
    assert_eq!(
        ui.input(UiInput::Key(HID_ESCAPE), &v).1,
        Some(UiCmd::DialogAnswer { id: 5, button: CLOSED, text: Some("10".into()) })
    );
    assert_eq!(ui.input(UiInput::Click(1.0, 1.0), &v), (true, None));
    assert_eq!(ui.input(UiInput::Other, &v), (true, None));
}

#[test]
fn a_dialog_without_a_field_is_steered_by_keys() {
    let v = view_with(Some(dialog(false, 2)));
    let mut ui = ModUi::new();
    ui.render(&v, 1, 1280, 720).unwrap();
    ui.input(UiInput::Char('x'), &v);
    assert_eq!(ui.dialog_text, "");
    ui.input(UiInput::Key(HID_RIGHT), &v);
    assert_eq!(ui.dialog_focus, 1);
    ui.input(UiInput::Key(HID_TAB), &v);
    ui.input(UiInput::Key(HID_LEFT), &v);
    assert_eq!(ui.dialog_focus, 1);
    ui.render(&v, 1, 1280, 720).unwrap();
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v).1, Some(UiCmd::DialogAnswer { id: 5, button: 1, text: None }));
    // A dialog without buttons can still be dismissed.
    let v = view_with(Some(dialog(false, 0)));
    ui.render(&v, 2, 1280, 720).unwrap();
    ui.input(UiInput::Key(HID_RIGHT), &v);
    assert_eq!(
        ui.input(UiInput::Key(HID_ESCAPE), &v).1,
        Some(UiCmd::DialogAnswer { id: 5, button: CLOSED, text: None })
    );
}
