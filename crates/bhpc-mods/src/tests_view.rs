use std::time::Instant;

use bhmod_proto::{Message, ModList, ServerHello};

use crate::host::{ModStatus, Usage};
use crate::join::{JoinFlow, Refusal};
use crate::tests_join::{cfg, info, pkg};
use crate::view::*;

#[test]
fn join_view_follows_the_flow() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let mut f = JoinFlow::new(cfg(d.path()));
    assert_eq!(JoinView::of(&f), None);
    f.contact(t);
    f.packet(
        &Message::ServerHello(ServerHello { proto: 1, mods_hash: [1; 32], client_required: true, count: 2 }).encode(),
        t,
    );
    let a = pkg("aa", "1.0.0", "", "", 0);
    let mut rich = info("aa", &a, false);
    rich.permissions.world = "read".into();
    rich.permissions.players = vec!["chat".into()];
    rich.permissions.net_http = vec!["x.org".into()];
    let mut nat = info("nn", b"x", true);
    nat.permissions.channels = false;
    nat.permissions.world = String::new();
    f.packet(
        &Message::ModList(ModList { mods: vec![rich, nat], channels: vec![], registry: Default::default() }).encode(),
        t,
    );
    let v = JoinView::of(&f).unwrap();
    assert_eq!(v.stage, JoinStage::Prompt);
    assert!(v.required);
    assert_eq!(v.mods[0].permissions, ["world:read", "players:chat", "net-http", "channels"]);
    assert!(v.mods[1].native && v.mods[1].refused && v.mods[1].permissions.is_empty());
    assert!(!v.can_accept(true));
    let mut ok = v.clone();
    ok.mods[1].refused = false;
    assert!(ok.can_accept(true) && !ok.can_accept(false));
    ok.mods.remove(1);
    assert!(ok.can_accept(false));
    // Declining required mods: nothing to show any more.
    f.decide(false, false, t);
    assert_eq!(JoinView::of(&f), None);
    let r = JoinView::rejected("S", "go away", Some("https://x"));
    assert_eq!(r.stage, JoinStage::Rejected("go away".into(), Some("https://x".into())));
    assert!(!r.can_accept(true));
    assert!(refusal_text(&Refusal::NativeNotListed).contains("public"));
    assert!(refusal_text(&Refusal::NativeBlocked).contains("Settings"));
}

#[test]
fn downloading_and_failed_views() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let a = pkg("aa", "1.0.0", "", "", 0);
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    f.packet(
        &Message::ServerHello(ServerHello { proto: 1, mods_hash: [1; 32], client_required: false, count: 1 }).encode(),
        t,
    );
    f.packet(
        &Message::ModList(ModList {
            mods: vec![info("aa", &a, false)],
            channels: vec![],
            registry: Default::default(),
        })
        .encode(),
        t,
    );
    f.decide(true, false, t);
    assert_eq!(JoinView::of(&f).unwrap().stage, JoinStage::Downloading);
    assert!(!JoinView::of(&f).unwrap().required);
    f.packet(&Message::FileError(bhmod_proto::FileError { sha256: crate::tests_join::sha(&a), code: 1 }).encode(), t);
    assert!(matches!(JoinView::of(&f).unwrap().stage, JoinStage::Failed(s) if s.contains("error 1")));
}

#[test]
fn status_line_is_json() {
    let s = ModStatus {
        id: "echo".into(),
        version: "1.0.0".into(),
        state: "running",
        reason: None,
        usage: Usage::default(),
    };
    let line = status_line(&[(s, "server")]);
    assert!(line.starts_with("@mods [{\"id\":\"echo\""), "{line}");
    let v: serde_json::Value = serde_json::from_str(line.strip_prefix("@mods ").unwrap()).unwrap();
    assert_eq!(v[0]["origin"], "server");
    assert_eq!(UiCmd::Dismiss, UiCmd::Dismiss);
    assert!(View::default().join.is_none());
}
