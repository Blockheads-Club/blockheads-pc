use std::time::Instant;

use bhmod_proto::{hex, Message, ModList, ServerHello};

use crate::host::{ModStatus, Usage};
use crate::join::{JoinFlow, Refusal};
use crate::tests_join::{cfg, info, pkg};
use crate::view::*;

#[test]
fn join_view_follows_the_flow() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let mut f = JoinFlow::new(cfg(d.path()));
    assert_eq!(JoinView::of(&f, t), None);
    f.contact(t);
    f.packet(
        &Message::ServerHello(ServerHello {
            proto: 1,
            mods_hash: [1; 32],
            client_required: true,
            count: 2,
            players: None,
            features: vec![],
            voice_udp_port: None,
        })
        .encode(),
        t,
    );
    let a = pkg("aa", "1.0.0", "", "", 0);
    let mut rich = info("aa", &a, false);
    rich.permissions.world = "read".into();
    rich.permissions.players = vec!["chat".into()];
    rich.permissions.net_http = vec!["x.org".into()];
    rich.permissions.voice = true;
    let mut nat = info("nn", b"x", true);
    nat.permissions.channels = false;
    nat.permissions.world = String::new();
    f.packet(
        &Message::ModList(ModList { mods: vec![rich, nat], channels: vec![], registry: Default::default() }).encode(),
        t,
    );
    let v = JoinView::of(&f, t).unwrap();
    assert_eq!(v.stage, JoinStage::Prompt);
    assert!(v.required);
    assert_eq!(v.mods[0].permissions, ["world:read", "players:chat", "net-http", "channels", "voice"]);
    assert!(v.mods[1].native && v.mods[1].refused && v.mods[1].permissions.is_empty());
    assert!(!v.can_accept(true));
    let mut ok = v.clone();
    ok.mods[1].refused = false;
    assert!(ok.can_accept(true) && !ok.can_accept(false));
    ok.mods.remove(1);
    assert!(ok.can_accept(false));
    // Declining required mods: nothing to show any more.
    f.decide(false, false, t);
    assert_eq!(JoinView::of(&f, t), None);
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
        &Message::ServerHello(ServerHello {
            proto: 1,
            mods_hash: [1; 32],
            client_required: false,
            count: 1,
            players: None,
            features: vec![],
            voice_udp_port: None,
        })
        .encode(),
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
    assert_eq!(JoinView::of(&f, t).unwrap().stage, JoinStage::Downloading);
    assert!(!JoinView::of(&f, t).unwrap().required);
    f.packet(&Message::FileError(bhmod_proto::FileError { sha256: crate::tests_join::sha(&a), code: 1 }).encode(), t);
    assert!(matches!(JoinView::of(&f, t).unwrap().stage, JoinStage::Failed(s) if s.text.contains("error 1")));
}

/// A flow at the prompt for `mods` (all required), already accepted.
fn accepted(d: &std::path::Path, mods: Vec<bhmod_proto::ModInfo>, t: Instant) -> JoinFlow {
    let mut f = JoinFlow::new(cfg(d));
    f.contact(t);
    let hello = ServerHello {
        proto: 1,
        mods_hash: [1; 32],
        client_required: true,
        count: mods.len() as u64,
        players: None,
        features: vec![],
        voice_udp_port: None,
    };
    f.packet(&Message::ServerHello(hello).encode(), t);
    f.packet(&Message::ModList(ModList { mods, channels: vec![], registry: Default::default() }).encode(), t);
    f.decide(true, false, t);
    f
}

fn chunk(bytes: &[u8], sha: [u8; 32], index: usize) -> Vec<u8> {
    let data = bytes.chunks(bhmod_proto::CHUNK_SIZE).nth(index).unwrap().to_vec();
    let offset = (index * bhmod_proto::CHUNK_SIZE) as u64;
    Message::FileChunk(bhmod_proto::FileChunk { sha256: sha, offset, total: bytes.len() as u64, data }).encode()
}

#[test]
fn failure_notices_and_the_pace() {
    use crate::view::FailKind;
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let big = pkg("big", "1.0.0", "", "", 3 * bhmod_proto::CHUNK_SIZE);
    let sha = crate::tests_join::sha(&big);
    let mut f = accepted(d.path(), vec![info("big", &big, false)], t);
    assert_eq!(f.address(), "127.0.0.1:15151");
    assert_eq!(f.eta_secs(t), None);
    f.packet(&chunk(&big, sha, 0), t);
    // No pace yet within the first second, then the pace so far: one chunk of ~4 in 2 s.
    assert_eq!(f.eta_secs(t + std::time::Duration::from_millis(500)), None);
    let left = f.eta_secs(t + std::time::Duration::from_secs(2)).unwrap();
    assert!((5..=7).contains(&left), "{left}");
    let v = JoinView::of(&f, t + std::time::Duration::from_secs(2)).unwrap();
    assert_eq!((v.address.as_str(), v.eta), ("127.0.0.1:15151", Some(left)));
    // Stopped: the share of the file that was there.
    let out = f.tick(t + crate::join::STALL + std::time::Duration::from_secs(1));
    assert_eq!(out, vec![crate::join::Action::Disconnect]);
    let JoinStage::Failed(fail) = JoinView::of(&f, t).unwrap().stage else { panic!() };
    assert!(
        matches!(&fail.kind, FailKind::Download { name, percent } if name == "Mod big" && (25..=40).contains(percent))
    );
    assert!(fail.text.contains("connection"));
    // A file whose bytes do not hash to the listed value.
    let d = tempfile::tempdir().unwrap();
    let small = pkg("small", "1.0.0", "", "", 0);
    let mut wrong = info("small", &small, false);
    wrong.sha256 = [9; 32];
    let mut f = accepted(d.path(), vec![wrong], t);
    f.packet(&chunk(&small, [9; 32], 0), t);
    let JoinStage::Failed(fail) = JoinView::of(&f, t).unwrap().stage else { panic!() };
    assert!(matches!(&fail.kind, FailKind::Check { name, version, expected, got }
        if name == "Mod small" && version == "1.0.0" && expected == &"09".repeat(32)
            && got.as_deref() == Some(hex(&crate::tests_join::sha(&small)).as_str())));
    // A package that needs a newer loader.
    let d = tempfile::tempdir().unwrap();
    let newer = pkg("newer", "1.0.0", "", "loader = \"^9.0\"", 0);
    let mut f = accepted(d.path(), vec![info("newer", &newer, false)], t);
    f.packet(&chunk(&newer, crate::tests_join::sha(&newer), 0), t);
    let JoinStage::Failed(fail) = JoinView::of(&f, t).unwrap().stage else { panic!() };
    assert_eq!(fail.kind, FailKind::Newer);
    assert!(fail.text.contains("newer mod loader"));
    // The server cannot send a file; the failure of an unknown id still reads.
    let d = tempfile::tempdir().unwrap();
    let mut f = accepted(d.path(), vec![info("small", &small, false)], t);
    let err = Message::FileError(bhmod_proto::FileError { sha256: crate::tests_join::sha(&small), code: 1 });
    f.packet(&err.encode(), t);
    let JoinStage::Failed(fail) = JoinView::of(&f, t).unwrap().stage else { panic!() };
    assert!(matches!(fail.kind, FailKind::Download { percent: 0, .. }) && fail.text.contains("error 1"));
    let none = crate::view::fail_for_tests(&bhmod_client::session::Failure::Stalled("zz".into()));
    assert!(matches!(&none.kind, FailKind::Download { name, percent: 0 } if name == "zz"));
    let hash = crate::view::fail_for_tests(&bhmod_client::session::Failure::HashMismatch("zz".into()));
    assert!(
        matches!(&hash.kind, FailKind::Check { version, expected, .. } if version.is_empty() && expected.is_empty())
    );
    // Anything else keeps its own words.
    let load = crate::view::fail_for_tests(&bhmod_client::session::Failure::LoadFailed("boom".into()));
    assert_eq!(load.kind, FailKind::Other);
    assert!(load.text.contains("boom"));
}

#[test]
fn status_line_is_json() {
    let s = ModStatus {
        id: "echo".into(),
        version: "1.0.0".into(),
        state: "running",
        reason: None,
        usage: Usage::default(),
        timings: crate::tests_ui::timings("echo", &[]),
    };
    let line = status_line(&[(s, "server")]);
    assert!(line.starts_with("@mods [{\"id\":\"echo\""), "{line}");
    let v: serde_json::Value = serde_json::from_str(line.strip_prefix("@mods ").unwrap()).unwrap();
    assert_eq!(v[0]["origin"], "server");
    assert_eq!(UiCmd::Dismiss, UiCmd::Dismiss);
    assert!(View::default().join.is_none());
}
