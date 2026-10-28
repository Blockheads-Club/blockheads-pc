//! The settings screen: rows from the settings contract, steps, drawing, keys and clicks; the microphone dialog.

use serde_json::json;

use crate::binds::BindRow;
use crate::form::*;
use crate::ui::*;
use crate::view::{UiCmd, View};
use crate::voice::Consent;

fn form() -> serde_json::Value {
    json!({ "fields": [
        { "key": "on", "type": "bool", "default": false, "label": { "en": "Enabled", "tr": "Açık" }, "restart": true },
        { "key": "radius", "type": "int", "default": 32, "label": "Radius", "min": 4, "max": 40, "step": 4,
          "help": { "tr": "Yarıçap" } },
        { "key": "gain", "type": "float", "default": 0.5, "label": { "tr": "Kazanç" } },
        { "key": "curve", "type": "enum", "default": "log", "values": ["linear", "log"] },
        { "key": "odd", "type": "enum", "default": "x" },
        { "key": "name", "type": "string", "default": "main" },
        { "key": "who", "type": "roles", "default": ["admin", "mod"] },
        { "key": "talk", "type": "matrix", "default": { "a": {}, "b": {} } },
        { "key": "ch", "type": "list", "default": [{ "n": 1 }] },
        { "key": "n", "type": "int", "default": 1, "label": 7 },
        { "key": "z", "type": "other", "default": 3 },
        { "type": "bool" }
    ]})
}

#[test]
fn rows_from_the_contract() {
    let rows = rows_of(Section::Server, &form(), &json!({ "on": true, "radius": 36 }));
    let texts: Vec<String> = rows.iter().map(|r| r.value.text()).collect();
    assert_eq!(texts, ["On", "36", "0.50", "log", "x", "main", "admin, mod", "2 rows", "1 entries", "1", "3"]);
    assert_eq!(rows[0].label, "Enabled");
    assert!(rows[0].restart);
    assert_eq!(rows[1].help.as_deref(), Some("Yarıçap"));
    assert_eq!(rows[2].label, "Kazanç");
    assert_eq!(rows[3].label, "curve");
    assert_eq!(rows[9].label, "n", "a label that is not text falls back to the key");
    // Steps.
    assert_eq!(rows[0].step(1), Some(json!(false)));
    assert_eq!(rows[1].step(1), Some(json!(40)));
    assert_eq!(rows[1].step(-1), Some(json!(32)));
    let top = Row {
        value: RowValue::Number { value: 40.0, min: Some(4.0), max: Some(40.0), step: 4.0, int: true },
        ..rows[1].clone()
    };
    assert_eq!(top.step(1), None);
    let bottom = Row {
        value: RowValue::Number { value: 4.0, min: Some(4.0), max: None, step: 4.0, int: true },
        ..rows[1].clone()
    };
    assert_eq!(bottom.step(-1), None);
    assert_eq!(rows[2].step(1), Some(json!(0.6)));
    assert_eq!(rows[3].step(1), Some(json!("linear")));
    assert_eq!(rows[3].step(-1), Some(json!("linear")));
    let unknown =
        Row { value: RowValue::Choice { value: "?".into(), values: vec!["a".into(), "b".into()] }, ..rows[3].clone() };
    assert_eq!(unknown.step(1), Some(json!("b")));
    assert_eq!(rows[5].step(1), None);
    assert!(rows_of(Section::Client, &json!({}), &json!({})).is_empty());
    // Keys and voice.
    let keys = key_rows(&[
        BindRow { action: "push-to_talk".into(), default_key: "v".into(), key: Some("f5".into()) },
        BindRow { action: "x".into(), default_key: "b".into(), key: None },
    ]);
    assert_eq!((keys[0].label.as_str(), keys[0].value.text()), ("push to talk", "F5".to_string()));
    assert_eq!(keys[1].value.text(), "—");
    assert_eq!(keys[1].step(1), None);
    let v = voice_rows(150, Consent::Refused);
    assert_eq!(
        (v[0].key.as_str(), v[0].value.text(), v[1].value.text()),
        (MASTER, "150".to_string(), "Off".to_string())
    );
    assert_eq!(voice_rows(100, Consent::Granted)[1].value, RowValue::Bool(true));
    for s in [Section::Voice, Section::Client, Section::Keys, Section::Server] {
        assert!(!s.title().is_empty());
    }
}

fn view(n_server: usize, status: Option<(bool, String)>) -> View {
    let mut rows = voice_rows(100, Consent::Unknown);
    rows.extend(rows_of(Section::Client, &form(), &json!({})).into_iter().take(4));
    rows.extend(key_rows(&[BindRow { action: "talk".into(), default_key: "v".into(), key: Some("v".into()) }]));
    let server = rows_of(Section::Server, &form(), &json!({}));
    rows.extend(server.into_iter().cycle().take(n_server));
    View {
        settings: Some(SettingsView { mod_id: "vm".into(), title: "Voice".into(), rows, status }),
        ..View::default()
    }
}

fn click(ui: &mut ModUi, v: &View, hit: Hit) -> (bool, Option<UiCmd>) {
    let r = ui.hits_for_tests().into_iter().find(|(_, h)| *h == hit).unwrap_or_else(|| panic!("no {hit:?}")).0;
    ui.input(UiInput::Click((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32), v)
}

#[test]
fn settings_screen_keys_and_clicks() {
    let v = view(10, None);
    let mut ui = ModUi::new();
    assert!(ui.modal(&v));
    assert!(ui.render(&v, 1, 1280, 720).is_some());
    assert!(ui.render(&v, 1, 1280, 720).is_some(), "cached");
    // Master volume: < and >.
    let (took, cmd) = click(&mut ui, &v, Hit::SettingInc(0));
    assert!(took);
    assert_eq!(
        cmd,
        Some(UiCmd::Setting { section: Section::Voice, mod_id: "vm".into(), key: MASTER.into(), value: json!(110) })
    );
    assert!(
        matches!(click(&mut ui, &v, Hit::SettingDec(0)).1, Some(UiCmd::Setting { value, .. }) if value == json!(90))
    );
    // The microphone switch.
    assert!(
        matches!(click(&mut ui, &v, Hit::SettingInc(1)).1, Some(UiCmd::Setting { value, .. }) if value == json!(true))
    );
    // Keyboard: down to the client `enum` row, change it.
    for _ in 0..5 {
        ui.input(UiInput::Key(HID_DOWN), &v);
    }
    assert!(
        matches!(ui.input(UiInput::Key(HID_RIGHT), &v).1, Some(UiCmd::Setting { value, .. }) if value == json!("linear"))
    );
    assert!(ui.input(UiInput::Key(HID_LEFT), &v).1.is_some());
    assert!(ui.input(UiInput::Key(HID_ENTER), &v).1.is_some());
    // The key row: Enter takes the next key.
    ui.input(UiInput::Key(HID_DOWN), &v);
    assert_eq!(ui.input(UiInput::Key(HID_SPACE), &v), (true, None));
    assert!(ui.capture);
    ui.render(&v, 1, 1280, 720).unwrap();
    assert_eq!(
        ui.input(UiInput::Key(9), &v).1,
        Some(UiCmd::Rebind { mod_id: "vm".into(), action: "talk".into(), key: Some("f".into()) })
    );
    ui.input(UiInput::Key(HID_ENTER), &v);
    assert_eq!(
        ui.input(UiInput::Key(HID_DELETE), &v).1,
        Some(UiCmd::Rebind { mod_id: "vm".into(), action: "talk".into(), key: None })
    );
    ui.input(UiInput::Key(HID_ENTER), &v);
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v), (true, None));
    ui.input(UiInput::Key(HID_ENTER), &v);
    assert_eq!(ui.input(UiInput::Key(50), &v), (true, None), "a key without a name");
    ui.input(UiInput::Key(HID_ENTER), &v);
    assert_eq!(ui.input(UiInput::Other, &v), (true, None), "anything else stops taking a key");
    assert!(!ui.capture);
    assert!(click(&mut ui, &v, Hit::SettingKey(6)).1.is_none());
    assert!(ui.capture);
    ui.input(UiInput::Key(HID_ESCAPE), &v);
    // Up wraps to the last row; the list scrolls so it shows.
    ui.sel = 0;
    ui.input(UiInput::Key(HID_UP), &v);
    assert_eq!(ui.sel, v.settings.as_ref().unwrap().rows.len() - 1);
    ui.render(&v, 2, 1280, 720).unwrap();
    assert!(ui.hits_for_tests().iter().all(|(_, h)| !matches!(h, Hit::SettingInc(0))), "scrolled past the first row");
    assert_eq!(ui.input(UiInput::Key(HID_F9), &v), (true, None));
    assert_eq!(ui.input(UiInput::Click(1.0, 1.0), &v), (true, None));
    assert_eq!(click(&mut ui, &v, Hit::CloseSettings).1, Some(UiCmd::CloseSettings));
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v).1, Some(UiCmd::CloseSettings));
    // Status lines, an empty screen, a short window.
    for (n, status) in [(0, Some((true, "Saved".to_string()))), (1, Some((false, "Too big".to_string())))] {
        let v = view(n, status);
        ui.render(&v, 3 + n as u64, 1280, 720).unwrap();
    }
    let empty = View { settings: Some(SettingsView::default()), ..View::default() };
    ui.render(&empty, 9, 640, 360).unwrap();
    assert_eq!(ui.input(UiInput::Key(HID_DOWN), &empty), (true, None));
    assert_eq!(ui.input(UiInput::Key(HID_RIGHT), &empty), (true, None));
}

#[test]
fn microphone_dialog() {
    let v = View { mic_ask: true, ..View::default() };
    let mut ui = ModUi::new();
    assert!(ui.modal(&v));
    ui.render(&v, 1, 1280, 720).unwrap();
    assert_eq!(click(&mut ui, &v, Hit::MicAllow).1, Some(UiCmd::MicConsent(true)));
    assert_eq!(click(&mut ui, &v, Hit::MicDeny).1, Some(UiCmd::MicConsent(false)));
    assert_eq!(ui.input(UiInput::Key(HID_ENTER), &v).1, Some(UiCmd::MicConsent(true)));
    assert_eq!(ui.input(UiInput::Key(HID_ESCAPE), &v).1, Some(UiCmd::MicConsent(false)));
    assert_eq!(ui.input(UiInput::Other, &v), (true, None));
}

#[test]
fn a_screen_from_its_sources() {
    let binds = [BindRow { action: "talk".into(), default_key: "v".into(), key: None }];
    let server = (form(), json!({}));
    let v = SettingsView::build(
        "vm",
        "Voice",
        Sources {
            voice: Some((100, Consent::Granted)),
            client: Some((json!({ "fields": [{ "key": "a", "type": "bool" }] }), json!({ "a": true }))),
            binds: &binds,
            server: Some(&server),
            players: vec![PlayerVolume { id: "id-a".into(), alias: "Ann".into(), volume: 150, muted: true }],
            widgets: Some(false),
        },
        Some((true, "Saved".into())),
    );
    let sections: Vec<Section> = v.rows.iter().map(|r| r.section).collect();
    assert_eq!(
        &sections[..8],
        &[
            Section::Voice,
            Section::Voice,
            Section::Players,
            Section::Players,
            Section::Client,
            Section::Hud,
            Section::Keys,
            Section::Server
        ]
    );
    assert_eq!(
        (v.rows[2].key.as_str(), v.rows[2].label.as_str(), v.rows[2].value.text()),
        ("@vol:id-a", "Ann", "150".into())
    );
    assert_eq!((v.rows[3].key.as_str(), v.rows[3].value.clone()), ("@mute:id-a", RowValue::Bool(true)));
    assert_eq!((v.rows[5].key.as_str(), v.rows[5].value.clone()), (WIDGETS, RowValue::Bool(false)));
    for s in [Section::Players, Section::Hud] {
        assert!(!s.title().is_empty());
    }
    assert_eq!((v.mod_id.as_str(), v.title.as_str(), v.status.unwrap().1.as_str()), ("vm", "Voice", "Saved"));
    assert!(SettingsView::build("x", "X", Sources::default(), None).rows.is_empty());
}

#[test]
fn server_forms_and_results() {
    let mut f = ServerForms::default();
    let s = bhmod_proto::SettingsSchema {
        mod_id: "vm".into(),
        schema: value_cbor(&form()),
        values: value_cbor(&json!({ "radius": 8 })),
    };
    assert!(f.receive(&s));
    assert_eq!(f.get("vm").unwrap().1["radius"], 8);
    assert!(!f.receive(&bhmod_proto::SettingsSchema { schema: vec![0xff], ..s }));
    f.clear();
    assert!(f.get("vm").is_none());
    let ok = bhmod_proto::SettingsResult { mod_id: Some("m".into()), key: None, outcome: Ok(()) };
    assert!(result_status(&ok).0);
    let bad = |message: Option<String>| bhmod_proto::SettingsResult {
        mod_id: None,
        key: Some("k".into()),
        outcome: Err(bhmod_proto::SettingsFailure { code: 3, message }),
    };
    assert_eq!(result_status(&bad(Some("too big".into()))), (false, "too big".to_string()));
    assert_eq!(result_status(&bad(None)), (false, "The server refused the change (error 3)".to_string()));
}
