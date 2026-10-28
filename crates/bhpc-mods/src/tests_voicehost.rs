//! The 0.3.4 client seams of the PC bridge: key bindings, overhead icons, the settings screen request, client
//! settings and the voice client as the mods' `voice` host.

use std::io::Write;
use std::path::Path;
use std::rc::Rc;

use bhmod_core::input::InputCommand;
use bhmod_core::types::ApiError;
use bhmod_core::ui::{OverheadIcon, UiCommand};
use bhmod_core::voice::{Icons, Mode, VoiceEvent};
use bhmod_core::GameBridge;

use crate::binds::{BindRow, Binds};
use crate::host::*;
use crate::join::Accepted;
use crate::overhead::{visible, Overheads};
use crate::session::{build, Plan};
use crate::tests_voice::FakeDriver;

const MAIN: &str = r#"
bh.events.on_custom("vm:input", function(ev) bh.log.info("input") end)
bh.events.on("chat", function(ev) bh.log.info("chat") end)
bh.events.on_custom("vm:ui-pick", function(ev) bh.log.info("picked") end)
bh.events.on_custom("voice:talk-start", function(ev) bh.log.info("talk-start") end)
bh.commands.register({ name = "ov" }, function()
    bh.ui.clear_overhead(bh.players.get("ME"))
    return { handled = true }
end)
bh.commands.register({ name = "vt" }, function()
    bh.voice.talk(nil, true)
    return { handled = true }
end)
return {
    on_enable = function()
        bh.input.bind("talk", "v")
        bh.input.bind("other", "k")
        bh.ui.set_overhead(bh.players.get("ME"), "speaking")
        bh.ui.open_settings("vm")
        bh.log.info("key " .. tostring(bh.input.key_of("talk")))
    end,
}
"#;

const SETTINGS: &str =
    "[permissions]\nvoice = true\n\n[settings.vol]\ntype = \"int\"\ndefault = 5\nmin = 0\nmax = 10\n\
label = \"Vol\"\nscope = \"client\"\n\n[settings.radius]\ntype = \"int\"\ndefault = 32\nlabel = \"Radius\"\n";

fn package() -> Vec<u8> {
    let toml = format!(
        "schema = 1\nid = \"vm\"\nversion = \"1.0.0\"\nname = {{ en = \"Voice\" }}\nauthors = [\"a\"]\n\
         license = \"MIT\"\napi = \"^0.3.4\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\n\
         path = \"client/main.luau\"\n{SETTINGS}"
    );
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("mod.toml", o).unwrap();
    w.write_all(toml.as_bytes()).unwrap();
    w.start_file("client/main.luau", o).unwrap();
    w.write_all(MAIN.as_bytes()).unwrap();
    w.finish().unwrap().into_inner()
}

fn host(dir: &Path, voice: Option<Rc<FakeDriver>>) -> ClientHost {
    let p = dir.join("vm.bhmod");
    std::fs::write(&p, package()).unwrap();
    let plan = Plan {
        own: None,
        dev: vec![],
        server: vec![Accepted { id: "vm".into(), path: p, grant: vec!["voice".into()] }],
    };
    let mods = dir.join("session");
    let (_, problems) = build(&mods, &plan).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    ClientHost::load(HostSetup {
        mods_dir: &mods,
        storage_dir: &dir.join("storage"),
        runtimes: bhmod_core::default_runtimes(),
        online: true,
        local_player: "ME".into(),
        voice: voice.map(|v| v as Rc<dyn crate::voice::VoiceDriver>),
        binds: Some(Binds::path(dir)),
        client_settings: Some(client_settings_path(dir)),
        hud: None,
    })
}

fn logs(h: &ClientHost) -> Vec<String> {
    h.take_new_logs().into_iter().map(|l| l.text).collect()
}

#[test]
fn a_voice_mod_on_the_pc_bridge() {
    let d = tempfile::tempdir().unwrap();
    let drv = Rc::new(FakeDriver::default());
    let mut h = host(d.path(), Some(drv.clone()));
    h.enable("World", "w");
    let l = logs(&h);
    assert!(l.contains(&"key v".to_string()), "{l:?}");
    assert_eq!(h.take_effects(), vec![Effect::OpenSettings("vm".into())]);
    // Overheads: drawn while the admin lets them show (all, when the voice client does not know yet).
    assert_eq!(h.overheads(), vec![("ME".to_string(), OverheadIcon::Speaking)]);
    drv.icons.set(Some(Icons { speaking: false, ..Icons::default() }));
    assert!(h.overheads().is_empty());
    // Key bindings.
    assert!(h.key("v", true));
    assert!(!h.key("x", true));
    assert!(logs(&h).contains(&"input".to_string()));
    assert_eq!(
        h.binds("vm"),
        vec![
            BindRow { action: "talk".into(), default_key: "v".into(), key: Some("v".into()) },
            BindRow { action: "other".into(), default_key: "k".into(), key: Some("k".into()) },
        ]
    );
    h.rebind("vm", "talk", Some("f5")).unwrap();
    assert!(!h.key("v", true));
    assert!(h.key("f5", false));
    assert!(std::fs::read_to_string(Binds::path(d.path())).unwrap().contains("f5"));
    // The voice client answers `voice.*`; its events reach the mods.
    assert!(h.typed("/vt"));
    assert_eq!(*drv.talk.borrow(), vec![(None, true)]);
    h.voice_events(vec![VoiceEvent::TalkStart { player: "ME".into(), mode: Mode::Proximity }]);
    assert!(logs(&h).contains(&"talk-start".to_string()));
    assert!(h.typed("/ov"));
    drv.icons.set(None);
    assert!(h.overheads().is_empty());
    // Client settings: only the client-scope field, stored on the device.
    let (form, values) = h.client_settings("vm").unwrap();
    assert_eq!(form["fields"].as_array().unwrap().len(), 1);
    assert_eq!(values["vol"], 5);
    h.set_client_setting("vm", "vol", &serde_json::json!(7)).unwrap();
    assert!(h.set_client_setting("vm", "radius", &serde_json::json!(7)).is_err());
    assert!(h.set_client_setting("vm", "vol", &serde_json::json!(70)).is_err());
    assert_eq!(h.client_settings("vm").unwrap().1["vol"], 7);
    assert!(std::fs::read_to_string(client_settings_path(d.path())).unwrap().contains("vol"));
    assert!(h.client_settings("nope").is_none());
    h.stop();
    assert!(!h.key("f5", true), "a stopped mod's bindings are gone");
}

#[test]
fn bridge_commands_without_voice() {
    let d = tempfile::tempdir().unwrap();
    let h = host(d.path(), None);
    let b = h.bridge_for_tests();
    assert!(b.voice().is_none());
    assert_eq!(b.ui("vm", &UiCommand::RemoveButton { id: "w".into() }), Err(ApiError::Unsupported));
    b.input("vm", &InputCommand::Bind { action: "a".into(), default_key: "q".into() }).unwrap();
    assert_eq!(b.input_key("vm", "a", "q"), Some("q".into()));
    b.input("vm", &InputCommand::Unbind { action: "a".into() }).unwrap();
    assert_eq!(b.input_key("vm", "a", "q"), None);
    b.ui("vm", &UiCommand::SetOverhead { player: "P".into(), icon: OverheadIcon::NoVoice }).unwrap();
    b.ui("vm", &UiCommand::ClearMod).unwrap();
    assert!(h.overheads().is_empty());
}

#[test]
fn binds_store() {
    let d = tempfile::tempdir().unwrap();
    let p = Binds::path(d.path());
    let mut b = Binds::load(&p);
    b.bind("m", "a", "v");
    b.bind("m", "a", "b");
    b.bind("n", "x", "b");
    assert_eq!(b.find("b"), vec![("m".into(), "a".into()), ("n".into(), "x".into())]);
    b.set_key("m", "a", None).unwrap();
    assert_eq!(b.key_of("m", "a"), None);
    assert!(b.find("b").len() == 1);
    b.set_key("m", "a", Some("b")).unwrap();
    assert_eq!(b.key_of("m", "a"), Some("b".into()));
    assert!(!std::fs::read_to_string(&p).unwrap().contains("[m]"), "back to the default: nothing stored");
    b.set_key("n", "x", Some("g")).unwrap();
    let mut again = Binds::load(&p);
    again.bind("n", "x", "b");
    assert_eq!(again.key_of("n", "x"), Some("g".into()));
    assert!(!again.unbind("n", "zz"));
    assert_eq!(again.key_of("zz", "x"), None);
    // Without a file nothing is written.
    let mut mem = Binds::default();
    mem.bind("m", "a", "v");
    mem.set_key("m", "a", Some("z")).unwrap();
    assert_eq!(mem.rows("m")[0].key.as_deref(), Some("z"));
    std::fs::write(&p, "not toml [").unwrap();
    assert!(Binds::load(&p).find("g").is_empty());
}

#[test]
fn overhead_priority_and_visibility() {
    let mut o = Overheads::default();
    assert!(o.is_empty());
    o.set("a", "p", OverheadIcon::Speaking);
    o.set("b", "p", OverheadIcon::AdminMute);
    o.set("a", "q", OverheadIcon::NoVoice);
    o.set("c", "q", OverheadIcon::Deafen);
    o.set("c", "r", OverheadIcon::SelfMute);
    let all = Icons::default();
    assert_eq!(
        o.shown(&all),
        vec![
            ("p".to_string(), OverheadIcon::AdminMute),
            ("q".to_string(), OverheadIcon::Deafen),
            ("r".to_string(), OverheadIcon::SelfMute)
        ]
    );
    let none = Icons { speaking: false, no_voice: false, self_mute: false, deafen: false, admin_mute: false };
    assert!(o.shown(&none).is_empty());
    assert_eq!(o.shown(&Icons { admin_mute: false, deafen: false, ..all })[0].1, OverheadIcon::Speaking);
    for i in crate::overhead::PRIORITY {
        assert!(visible(i, &all));
        assert!(!visible(i, &none));
    }
    assert!(o.clear("a", "p"));
    assert!(!o.clear("a", "p"));
    o.clear_mod("c");
    assert_eq!(
        o.shown(&all),
        vec![("p".to_string(), OverheadIcon::AdminMute), ("q".to_string(), OverheadIcon::NoVoice)]
    );
}

#[test]
fn players_by_id_once_voice_lists_them() {
    let d = tempfile::tempdir().unwrap();
    let drv = Rc::new(FakeDriver::default());
    let mut h = host(d.path(), Some(drv.clone()));
    let b = h.bridge_for_tests();
    assert_eq!(b.local_player(), Some("ME".into()));
    assert!(b.platform().starts_with("pc-"));
    assert_eq!(b.online_players(), vec!["ME".to_string()]);
    h.set_local_id(Some("id-me".into()));
    assert_eq!(b.local_player(), Some("id-me".into()));
    assert_eq!(b.player_alias("id-me"), Some("ME".into()));
    *drv.names.borrow_mut() = vec![("id-a".into(), "Ann".into())];
    assert_eq!(b.online_players(), vec!["id-me".to_string(), "id-a".to_string()]);
    assert_eq!(h.online(), b.online_players());
    assert_eq!(b.player_alias("id-a"), Some("Ann".into()));
    assert_eq!(b.player_alias("nobody"), None);
    assert!(b.send_chat("id-me", "hi") && b.send_chat("ME", "hi") && !b.send_chat("id-a", "hi"));
    h.set_local_id(Some(String::new()));
    assert_eq!(b.local_player(), Some("ME".into()));
    *drv.names.borrow_mut() = vec![];
    h.enable("World", "w");
    let _ = logs(&h);
    h.pick("ME");
    assert!(logs(&h).contains(&"picked".to_string()));
}

#[test]
fn picking_a_blockhead() {
    use crate::overhead::{pick, HEAD_LIFT};
    let heads = vec![("a".to_string(), (10.0, 5.0 + HEAD_LIFT)), ("b".to_string(), (10.4, 5.0 + HEAD_LIFT))];
    assert_eq!(pick(&heads, (10.35, 6.0)), Some("b".into()));
    assert_eq!(pick(&heads, (9.8, 5.0)), Some("a".into()));
    assert_eq!(pick(&heads, (10.0, 8.0)), None, "above the head");
    assert_eq!(pick(&heads, (12.0, 6.0)), None);
    assert_eq!(pick(&[], (0.0, 0.0)), None);
}

#[test]
fn players_are_named_by_vanilla_id_once_known() {
    let d = tempfile::tempdir().unwrap();
    let drv = Rc::new(FakeDriver::default());
    let mut h = host(d.path(), Some(drv.clone()));
    let b = h.bridge_for_tests();
    h.enable("World", "w");
    // Before the ids are known: aliases, as before.
    h.players(vec!["Ann".into()]);
    assert_eq!(b.online_players(), vec!["ME".to_string(), "Ann".to_string()]);
    // The server's list gives ids; VOICE_STATE knows Bob; the local player has localPlayerID.
    h.set_ids(vec![("id-ann".into(), "Ann".into()), (String::new(), "x".into())]);
    *drv.names.borrow_mut() = vec![("id-bob".into(), "Bob".into())];
    h.set_local_id(Some("id-me".into()));
    assert_eq!(b.online_players(), vec!["id-me".to_string(), "id-ann".to_string(), "id-bob".to_string()]);
    assert_eq!(b.player_alias("id-ann"), Some("Ann".into()));
    h.players(vec!["Ann".into(), "Bob".into()]);
    assert_eq!(b.online_players(), vec!["id-me".to_string(), "id-ann".to_string(), "id-bob".to_string()]);
    // World events and positions use ids too.
    h.moved("Ann", (1, 2));
    assert_eq!(b.player_pos("id-ann"), Some(bhmod_core::types::Pos { x: 1, y: 2 }));
    h.moved("ME", (3, 4));
    assert_eq!(b.player_pos("id-me"), Some(bhmod_core::types::Pos { x: 3, y: 4 }));
    h.block_placed("Ann", (5, 5), 1, (0, 0));
    h.block_broken("Ann", (5, 5), false);
    h.object_placed("Ann", (5, 5), true, 3);
    h.damaged(Some("Ann"), 1, 1.0);
    h.died(Some("Ann"), 1);
    assert_eq!(h.chat_received("Ann", "hi"), Some("hi".into()));
    assert_eq!(h.chat_received("", "server"), Some("server".into()));
}
