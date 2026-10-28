use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use bhmod_client::session::Failure;
use bhmod_proto::{
    file_error, hex, ChannelInfo, ChannelRegister, FileChunk, FileError, FileRequest, Message, ModInfo, ModList,
    ModPermissions, RejectText, ServerHello, CHUNK_SIZE,
};
use bhpc_launcher::mods::index::parse_index;
use bhpc_launcher::mods::ModsDirs;
use sha2::{Digest, Sha256};

use crate::join::*;

/// A client mod package; `pad` bytes of incompressible data make it span several chunks.
pub(crate) fn pkg(id: &str, version: &str, main: &str, extra: &str, pad: usize) -> Vec<u8> {
    let toml = format!(
        "schema = 1\nid = \"{id}\"\nversion = \"{version}\"\nname = {{ en = \"Mod {id}\" }}\nauthors = [\"a\"]\n\
         license = \"MIT\"\napi = \"^0.2\"\n{extra}\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\n\
         path = \"client/main.luau\"\n"
    );
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("mod.toml", o).unwrap();
    w.write_all(toml.as_bytes()).unwrap();
    if extra.contains("[provides]") {
        w.start_file("wit/svc.wit", o).unwrap();
        w.write_all(b"package x:y;\n").unwrap();
    }
    w.start_file("client/main.luau", o).unwrap();
    w.write_all(if main.is_empty() { "return {}\n" } else { main }.as_bytes()).unwrap();
    if pad > 0 {
        let mut x: u32 = 1;
        let data: Vec<u8> = (0..pad)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        w.start_file("data/pad.bin", o).unwrap();
        w.write_all(&data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

pub(crate) fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

pub(crate) fn info(id: &str, bytes: &[u8], native: bool) -> ModInfo {
    ModInfo {
        id: id.into(),
        version: "1.0.0".into(),
        name: [("en".to_string(), format!("Mod {id}"))].into(),
        description: Default::default(),
        sides: vec!["client".into()],
        client_required: true,
        sha256: sha(bytes),
        size: bytes.len() as u64,
        permissions: ModPermissions {
            world: "none".into(),
            players: vec![],
            net_http: vec![],
            channels: true,
            voice: false,
            native,
        },
        source_url: None,
        index_listed: false,
    }
}

pub(crate) fn cfg(dir: &Path) -> JoinConfig {
    JoinConfig {
        loader: "0.1.0".into(),
        platform: crate::platform(),
        api: "0.2.0".into(),
        runtimes: vec!["lua".into(), "wasm".into()],
        features: crate::voice::features(true),
        dirs: ModsDirs::new(dir),
        index: None,
        block_native: false,
        address: "127.0.0.1:15151".into(),
    }
}

/// The same mod, but the client may run without it.
pub(crate) fn optional(mut i: ModInfo) -> ModInfo {
    i.client_required = false;
    i
}

fn hello(count: u64, required: bool) -> Vec<u8> {
    Message::ServerHello(ServerHello {
        proto: 1,
        mods_hash: [7; 32],
        client_required: required,
        count,
        players: None,
        features: vec![],
        voice_udp_port: None,
    })
    .encode()
}

fn list(mods: Vec<ModInfo>) -> Vec<u8> {
    Message::ModList(ModList {
        mods,
        channels: vec![ChannelInfo { name: "echo:ping".into(), id: 3 }],
        registry: [("cosmetics".to_string(), [("echo:crown".to_string(), 1u64)].into())].into(),
    })
    .encode()
}

fn chunks(bytes: &[u8]) -> Vec<Vec<u8>> {
    bytes
        .chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(i, d)| {
            Message::FileChunk(FileChunk {
                sha256: sha(bytes),
                offset: (i * CHUNK_SIZE) as u64,
                total: bytes.len() as u64,
                data: d.to_vec(),
            })
            .encode()
        })
        .collect()
}

fn sent(a: &[Action]) -> Vec<Message> {
    a.iter()
        .filter_map(|x| match x {
            Action::Send(b) => Some(Message::decode(b).unwrap()),
            _ => None,
        })
        .collect()
}

/// A flow at the prompt with `mods`.
fn at_prompt(dir: &Path, mods: Vec<ModInfo>, c: JoinConfig) -> (JoinFlow, Instant) {
    let now = Instant::now();
    let mut f = JoinFlow::new(c);
    let _ = dir;
    assert!(matches!(sent(&f.contact(now))[0], Message::Hello(_)));
    assert_eq!(f.player_info(), PlayerInfo::Hold);
    assert_eq!(sent(&f.packet(&hello(mods.len() as u64, true), now)), vec![Message::ListRequest]);
    assert_eq!(f.packet(&list(mods), now), vec![Action::Prompt]);
    assert_eq!(f.stage(), Stage::Prompt);
    (f, now)
}

#[test]
fn vanilla_server_times_out_and_releases_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let mut f = JoinFlow::new(cfg(d.path()));
    assert_eq!(f.stage(), Stage::Idle);
    let t = Instant::now();
    let a = f.contact(t);
    let Message::Hello(h) = &sent(&a)[0] else { panic!() };
    assert_eq!((h.proto, h.cached.len()), (1, 0));
    assert_eq!(f.stage(), Stage::Hello);
    assert!(f.contact(t).is_empty());
    assert_eq!(f.player_info(), PlayerInfo::Hold);
    assert!(f.tick(t + Duration::from_millis(1499)).is_empty());
    assert_eq!(f.tick(t + Duration::from_millis(1500)), vec![Action::Release(None)]);
    assert_eq!(f.stage(), Stage::Vanilla);
    // Late or stray packets change nothing; the 0x1f goes out unchanged from now on.
    assert!(f.packet(&hello(0, false), t).is_empty());
    assert!(f.packet(&[0xbc, 0xff], t).is_empty());
    assert_eq!(f.player_info(), PlayerInfo::Send(None));
    assert!(f.tick(t + Duration::from_secs(9)).is_empty());
    // Runtimes this loader does not have are not announced to the mods' checks.
    let mut c = cfg(d.path());
    c.runtimes = vec!["lua".into(), "jvm".into()];
    c.loader = "dev-build".into();
    assert!(
        matches!(&sent(&JoinFlow::new(c).contact(t))[0], Message::Hello(h) if h.runtimes == ["lua", "jvm"] && h.loader == "dev-build")
    );
    // A game that sends before any 0x23 is treated like vanilla.
    let mut g = JoinFlow::new(cfg(d.path()));
    assert_eq!(g.player_info(), PlayerInfo::Send(None));
}

#[test]
fn server_without_mods_joins_with_the_hash() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    f.player_info();
    let a = f.packet(&hello(0, false), t);
    assert_eq!(a, vec![Action::Release(Some(hex(&[7; 32])))]);
    assert_eq!(f.stage(), Stage::Joined);
    assert_eq!(f.player_info(), PlayerInfo::Send(Some(hex(&[7; 32]))));
    assert!(f.entries().is_empty());
}

#[test]
fn full_download_with_acks_resume_and_cache() {
    let d = tempfile::tempdir().unwrap();
    let big = pkg("big", "1.0.0", "return {}\n", "[permissions]\nchannels = true", 5 * CHUNK_SIZE + 100);
    let small = pkg("small", "1.0.0", "return {}\n", "", 0);
    // A partial file from an earlier attempt: 1.5 chunks, resumed from the first whole chunk.
    let blobs = d.path().join("mods-cache/blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(format!("{}.part", hex(&sha(&big)))), &big[..CHUNK_SIZE + CHUNK_SIZE / 2]).unwrap();
    let (mut f, t) = at_prompt(d.path(), vec![info("big", &big, false), info("small", &small, false)], cfg(d.path()));
    assert!(f.acceptable() && !f.has_native() && f.required());
    assert_eq!(f.entries()[0].name(), "Mod big");
    assert_eq!(f.progress(), (0, (big.len() + small.len()) as u64));
    // Wrong stage for a decision before; the accept starts at offset 32768.
    let a = f.decide(true, false, t);
    assert_eq!(sent(&a), vec![Message::FileRequest(FileRequest { sha256: sha(&big), offset: CHUNK_SIZE as u64 })]);
    assert_eq!(f.stage(), Stage::Downloading);
    assert!(f.decide(true, false, t).is_empty());
    assert_eq!(f.entries()[0].have, CHUNK_SIZE as u64);
    let cs = chunks(&big);
    // A chunk for another file is ignored.
    let mut other = cs[2].clone();
    other[2] ^= 1;
    assert!(f.packet(&other, t).is_empty());
    let mut acks = Vec::new();
    for c in &cs[1..cs.len() - 1] {
        acks.extend(sent(&f.packet(c, t)));
    }
    // 4 chunks since the request: one acknowledgement at offset 5 * 32768.
    assert_eq!(acks, vec![Message::FileRequest(FileRequest { sha256: sha(&big), offset: 5 * CHUNK_SIZE as u64 })]);
    assert_eq!(f.player_info(), PlayerInfo::Hold);
    let a = f.packet(cs.last().unwrap(), t);
    let msgs = sent(&a);
    assert_eq!(msgs[0], Message::FileRequest(FileRequest { sha256: sha(&big), offset: big.len() as u64 }));
    assert_eq!(msgs[1], Message::FileRequest(FileRequest { sha256: sha(&small), offset: 0 }));
    assert!(f.entries()[0].done && !f.entries()[1].done);
    let a = f.packet(&chunks(&small)[0], t);
    let msgs = sent(&a);
    assert!(matches!(&msgs[1], Message::Ready(r) if r.accepted_hash == [7; 32] && r.loaded.len() == 2));
    assert!(a.contains(&Action::Release(Some(hex(&[7; 32])))));
    let Some(Action::Run(run)) = a.last() else { panic!("{a:?}") };
    assert_eq!(run.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["big", "small"]);
    assert_eq!(run[0].grant, ["channels"]);
    assert!(run[0].path.is_file());
    assert!(!blobs.join(format!("{}.part", hex(&sha(&big)))).exists());
    assert_eq!(f.stage(), Stage::Joined);
    // The launcher's cache record.
    let rec = std::fs::read_to_string(d.path().join("mods-cache/servers/127.0.0.1_15151.toml")).unwrap();
    assert!(rec.contains("Mod big"), "{rec}");
    // The same server again with the same files: no prompt, the same mods are taken.
    let mut k = JoinFlow::new(cfg(d.path()));
    k.contact(t);
    k.player_info();
    k.packet(&hello(2, true), t);
    let a = k.packet(&list(vec![info("big", &big, false), info("small", &small, false)]), t);
    assert!(!a.contains(&Action::Prompt), "{a:?}");
    assert!(k.entries().iter().all(|e| e.known && e.enabled));
    assert!(matches!(&sent(&a)[0], Message::Ready(_)), "{a:?}");
    // A changed mod is asked about again; the unchanged one is not listed.
    let mut n = JoinFlow::new(cfg(d.path()));
    n.contact(t);
    n.player_info();
    n.packet(&hello(2, true), t);
    let a = n.packet(&list(vec![info("big", &big, false), info("small", b"changed", false)]), t);
    assert!(a.contains(&Action::Prompt), "{a:?}");
    assert_eq!(n.entries().iter().map(|e| e.known).collect::<Vec<_>>(), [true, false]);
    // A second join finds both in the cache: HELLO lists them and nothing downloads.
    let mut c = cfg(d.path());
    c.address = "other:1".into();
    let mut g = JoinFlow::new(c);
    g.set_server_name("Sunny Isle");
    assert_eq!(g.server_name(), "Sunny Isle");
    let Message::Hello(h) = &sent(&g.contact(t))[0] else { panic!() };
    assert_eq!(h.cached.len(), 2);
    g.player_info();
    g.packet(&hello(2, true), t);
    g.packet(&list(vec![info("big", &big, false), info("small", &small, false)]), t);
    assert!(g.entries().iter().all(|e| e.cached && e.done));
    assert_eq!(g.progress(), (0, 0));
    let a = g.decide(true, false, t);
    assert!(matches!(&sent(&a)[0], Message::Ready(_)));
    // Channels: from MOD_LIST and CHANNEL_REGISTER; unknown ids are dropped.
    let reg = Message::ChannelRegister(ChannelRegister { name: "echo:late".into(), id: 9 }).encode();
    assert!(g.packet(&reg, t).is_empty());
    let pkt = g.channel_packet("echo:late", b"hi").unwrap();
    assert_eq!(pkt, Message::Channel(bhmod_proto::Channel { channel_id: 9, payload: b"hi".to_vec() }).encode());
    assert_eq!(g.channel_packet("nope", b""), None);
    let ch = |id| Message::Channel(bhmod_proto::Channel { channel_id: id, payload: b"yo".to_vec() }).encode();
    assert_eq!(g.packet(&ch(3), t), vec![Action::ChannelIn("echo:ping".into(), b"yo".to_vec())]);
    assert!(g.packet(&ch(4), t).is_empty());
    // The registry of MOD_LIST is kept; COSMETICS is handed to the glue after the join.
    assert_eq!(g.registry()["cosmetics"]["echo:crown"], 1);
    let wear =
        bhmod_proto::Cosmetics { full: true, players: [("p".to_string(), [("hat".to_string(), 1u64)].into())].into() };
    assert_eq!(g.packet(&Message::Cosmetics(wear.clone()).encode(), t), vec![Action::Cosmetics(wear)]);
    // Service calls: the server's call and answer reach the glue; the client's packets are built.
    let call = bhmod_proto::ServiceCall {
        id: 5,
        mod_id: "srv".into(),
        service: "hud".into(),
        version_req: "^1".into(),
        method: "title".into(),
        args: vec![0x80],
    };
    assert_eq!(g.packet(&Message::ServiceCall(call.clone()).encode(), t), vec![Action::ServiceCallIn(call.clone())]);
    let result = bhmod_proto::ServiceResult { id: 5, outcome: Ok(vec![0xf6]) };
    assert_eq!(
        g.packet(&Message::ServiceResult(result.clone()).encode(), t),
        vec![Action::ServiceResultIn(result.clone())]
    );
    let out = bhmod_core::RemoteCall {
        id: 8,
        target: bhmod_core::RemoteTarget::Server,
        mod_id: "echo".into(),
        service: "prices".into(),
        version_req: "*".into(),
        method: "price".into(),
        args: vec![0x80],
    };
    let pkt = g.service_call_packet(&out).unwrap();
    assert!(matches!(Message::decode(&pkt).unwrap(), Message::ServiceCall(c) if c.id == 8 && c.mod_id == "echo"));
    let pkt = g.service_result_packet(result.clone()).unwrap();
    assert_eq!(Message::decode(&pkt).unwrap(), Message::ServiceResult(result));
}

#[test]
fn channels_need_a_finished_join() {
    let d = tempfile::tempdir().unwrap();
    let small = pkg("small", "1.0.0", "", "", 0);
    let (mut f, t) = at_prompt(d.path(), vec![info("small", &small, false)], cfg(d.path()));
    assert_eq!(f.channel_packet("echo:ping", b""), None);
    let ch = Message::Channel(bhmod_proto::Channel { channel_id: 3, payload: vec![] }).encode();
    assert!(f.packet(&ch, t).is_empty());
    // Nor do service calls.
    let call = bhmod_proto::ServiceCall {
        id: 1,
        mod_id: "srv".into(),
        service: "s".into(),
        version_req: "*".into(),
        method: "m".into(),
        args: vec![0x80],
    };
    assert!(f.packet(&Message::ServiceCall(call.clone()).encode(), t).is_empty());
    let result = bhmod_proto::ServiceResult { id: 1, outcome: Ok(vec![0xf6]) };
    assert!(f.packet(&Message::ServiceResult(result.clone()).encode(), t).is_empty());
    let out = bhmod_core::RemoteCall {
        id: 1,
        target: bhmod_core::RemoteTarget::Server,
        mod_id: "echo".into(),
        service: "s".into(),
        version_req: "*".into(),
        method: "m".into(),
        args: vec![0x80],
    };
    assert_eq!(f.service_call_packet(&out), None);
    assert_eq!(f.service_result_packet(result), None);
}

#[test]
fn busy_retry_stall_and_file_errors() {
    let d = tempfile::tempdir().unwrap();
    let a = pkg("aa", "1.0.0", "", "", 2 * CHUNK_SIZE);
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    let err = |code| Message::FileError(FileError { sha256: sha(&a), code }).encode();
    // Another file's error is ignored; BUSY retries after a pause.
    assert!(f.packet(&Message::FileError(FileError { sha256: [1; 32], code: 1 }).encode(), t).is_empty());
    assert!(f.packet(&err(file_error::BUSY), t).is_empty());
    assert!(f.tick(t + Duration::from_secs(1)).is_empty());
    let again = f.tick(t + BUSY_RETRY);
    assert_eq!(sent(&again), vec![Message::FileRequest(FileRequest { sha256: sha(&a), offset: 0 })]);
    // No data for STALL: failed.
    f.packet(&chunks(&a)[0], t + BUSY_RETRY);
    assert!(f.tick(t + BUSY_RETRY + Duration::from_secs(1)).is_empty());
    assert_eq!(f.tick(t + BUSY_RETRY + STALL), vec![Action::Disconnect]);
    assert_eq!(f.stage(), Stage::Failed(Failure::Stalled("aa".into())));
    // NOT_FOUND fails at once.
    let d2 = tempfile::tempdir().unwrap();
    let (mut g, t) = at_prompt(d2.path(), vec![info("aa", &a, false)], cfg(d2.path()));
    g.decide(true, false, t);
    assert_eq!(g.packet(&err(file_error::NOT_FOUND), t), vec![Action::Disconnect]);
    assert_eq!(g.stage(), Stage::Failed(Failure::Download { id: "aa".into(), code: 1 }));
    // A failed join sends the game's 0x1f unchanged (the connection is closing anyway).
    assert_eq!(g.player_info(), PlayerInfo::Send(None));
}

#[test]
fn bad_chunks_checksums_and_package_rules() {
    let d = tempfile::tempdir().unwrap();
    let a = pkg("aa", "1.0.0", "", "", 0);
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    let bad = Message::FileChunk(FileChunk { sha256: sha(&a), offset: 0, total: a.len() as u64 + 5, data: vec![1] });
    assert_eq!(f.packet(&bad.encode(), t), vec![Action::Disconnect]);
    assert!(matches!(f.stage(), Stage::Failed(Failure::Protocol(_))));
    // The server lists a hash the bytes do not have.
    let d = tempfile::tempdir().unwrap();
    let mut wrong = info("aa", &a, false);
    wrong.sha256 = [9; 32];
    let (mut f, t) = at_prompt(d.path(), vec![wrong], cfg(d.path()));
    f.decide(true, false, t);
    let c = Message::FileChunk(FileChunk { sha256: [9; 32], offset: 0, total: a.len() as u64, data: a.clone() });
    assert_eq!(f.packet(&c.encode(), t), vec![Action::Disconnect]);
    assert_eq!(f.stage(), Stage::Failed(Failure::HashMismatch("aa".into())));
    // Not a package (right hash, broken zip): refused by the package rules.
    let d = tempfile::tempdir().unwrap();
    let junk = b"not a zip".to_vec();
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &junk, false)], cfg(d.path()));
    f.decide(true, false, t);
    assert_eq!(f.packet(&chunks(&junk)[0], t).last(), Some(&Action::Disconnect));
    let Stage::Failed(e @ Failure::Refused { .. }) = f.stage() else { panic!() };
    assert!(e.to_string().starts_with("aa: "));
    // A valid package listed under another id.
    let d = tempfile::tempdir().unwrap();
    let (mut f, t) = at_prompt(d.path(), vec![info("bb", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    f.packet(&chunks(&a)[0], t);
    assert!(matches!(f.stage(), Stage::Failed(Failure::Refused { .. })), "{:?}", f.stage());
    // A cached file whose name lies about its hash is deleted and not run.
    let d = tempfile::tempdir().unwrap();
    let blobs = d.path().join("mods-cache/blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    let lie_path = blobs.join(format!("{}.bhmod", hex(&[5; 32])));
    std::fs::write(&lie_path, &a).unwrap();
    let mut lie = info("aa", &a, false);
    lie.sha256 = [5; 32];
    let (mut f, t) = at_prompt(d.path(), vec![lie], cfg(d.path()));
    assert_eq!(f.decide(true, false, t).last(), Some(&Action::Disconnect));
    assert!(matches!(f.stage(), Stage::Failed(Failure::LoadFailed(m)) if m.contains("not 0505")));
    assert!(!lie_path.exists());
}

#[test]
fn cache_write_failures() {
    let d = tempfile::tempdir().unwrap();
    let a = pkg("aa", "1.0.0", "", "", 0);
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    // The blobs folder is a file: the .part cannot be written.
    std::fs::create_dir_all(d.path().join("mods-cache")).unwrap();
    std::fs::write(d.path().join("mods-cache/blobs"), b"").unwrap();
    assert_eq!(f.packet(&chunks(&a)[0], t), vec![Action::Disconnect]);
    assert!(matches!(f.stage(), Stage::Failed(e @ Failure::Disk(_)) if e.to_string().contains("save")));
    // The finished file cannot be stored (a folder in its place).
    let d = tempfile::tempdir().unwrap();
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    std::fs::create_dir_all(d.path().join(format!("mods-cache/blobs/{}.bhmod", hex(&sha(&a))))).unwrap();
    assert_eq!(f.packet(&chunks(&a)[0], t).last(), Some(&Action::Disconnect));
    assert!(matches!(f.stage(), Stage::Failed(Failure::Disk(_))));
}

#[test]
fn decline_required_and_optional() {
    let d = tempfile::tempdir().unwrap();
    let a = pkg("aa", "1.0.0", "", "", 0);
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    assert_eq!(f.decide(false, false, t), vec![Action::Disconnect]);
    assert_eq!(f.stage(), Stage::Declined);
    assert_eq!(f.player_info(), PlayerInfo::Send(None));
    // Optional client mods only: declining joins without them (still with the hash).
    let t = Instant::now();
    let mut g = JoinFlow::new(cfg(d.path()));
    g.contact(t);
    g.player_info();
    g.packet(&hello(1, false), t);
    g.packet(&list(vec![optional(info("aa", &a, false))]), t);
    assert!(!g.required());
    let acts = g.decide(false, false, t);
    assert_eq!(acts, vec![Action::Release(Some(hex(&[7; 32])))]);
    // A list with only server mods needs no prompt.
    let mut h = JoinFlow::new(cfg(d.path()));
    h.contact(t);
    let mut srv = info("s", b"x", false);
    srv.sides = vec!["server".into()];
    assert!(h.packet(&hello(1, false), t).iter().all(|a| matches!(a, Action::Send(_))));
    assert!(h.packet(&list(vec![srv]), t).is_empty());
    assert_eq!(h.stage(), Stage::Joined);
    assert!(h.entries().is_empty());
}

#[test]
fn optional_mods_are_switched_per_mod() {
    let d = tempfile::tempdir().unwrap();
    let (req, aa, bb) = (pkg("req", "1.0.0", "", "", 0), pkg("aa", "1.0.0", "", "", 0), pkg("bb", "1.0.0", "", "", 0));
    let mods = vec![info("req", &req, false), optional(info("aa", &aa, false)), optional(info("bb", &bb, false))];
    let (mut f, t) = at_prompt(d.path(), mods, cfg(d.path()));
    assert!(f.entries().iter().all(|e| e.enabled));
    assert!(f.entries()[0].required() && !f.entries()[1].required());
    let all = (req.len() + aa.len() + bb.len()) as u64;
    assert_eq!(f.progress(), (0, all));
    // Required mods stay on; an optional one can be switched off and on; an index out of range is ignored.
    f.toggle(0);
    f.toggle(9);
    assert!(f.entries()[0].enabled);
    f.toggle(1);
    assert!(!f.entries()[1].enabled);
    assert_eq!(f.entries()[1].to_download(), 0);
    assert_eq!(f.progress(), (0, all - aa.len() as u64));
    f.toggle(1);
    f.toggle(1);
    assert_eq!(f.progress(), (0, all - aa.len() as u64));
    // Accepting downloads and runs the required mod and the optional one that stayed on.
    let a = f.decide(true, false, t);
    assert_eq!(sent(&a), vec![Message::FileRequest(FileRequest { sha256: sha(&req), offset: 0 })]);
    f.toggle(2);
    assert!(f.entries()[2].enabled, "no switching while downloading");
    let a = f.packet(&chunks(&req)[0], t);
    assert_eq!(sent(&a)[1], Message::FileRequest(FileRequest { sha256: sha(&bb), offset: 0 }));
    let a = f.packet(&chunks(&bb)[0], t);
    let msgs = sent(&a);
    let Message::Ready(ready) = &msgs[1] else { panic!("{msgs:?}") };
    assert_eq!(ready.loaded.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["req", "bb"]);
    let Some(Action::Run(run)) = a.last() else { panic!("{a:?}") };
    assert_eq!(run.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["req", "bb"]);
    f.toggle(1);
    assert!(!f.entries()[1].enabled, "no switching after the prompt");
}

#[test]
fn a_refused_optional_mod_stays_off_and_does_not_block() {
    let d = tempfile::tempdir().unwrap();
    let (req, nat) = (pkg("req", "1.0.0", "", "", 0), pkg("nat", "1.0.0", "", "[permissions]\nnative = true", 0));
    let (mut f, t) =
        at_prompt(d.path(), vec![info("req", &req, false), optional(info("nat", &nat, true))], cfg(d.path()));
    assert_eq!(f.entries()[1].refusal, Some(Refusal::NativeNotListed));
    assert!(!f.entries()[1].enabled);
    f.toggle(1);
    assert!(!f.entries()[1].enabled);
    // Not taken, so it neither blocks "Download and join" nor needs the red confirmation.
    assert!(f.acceptable() && !f.has_native());
    assert!(matches!(&sent(&f.decide(true, false, t))[0], Message::FileRequest(r) if r.sha256 == sha(&req)));
}

#[test]
fn native_mods_need_the_index_and_a_confirmation() {
    let d = tempfile::tempdir().unwrap();
    let n = pkg("nat", "1.0.0", "", "[permissions]\nnative = true", 0);
    // Not listed: refused (accepting is impossible and counts as declining).
    let (mut f, t) = at_prompt(d.path(), vec![info("nat", &n, true)], cfg(d.path()));
    assert_eq!(f.entries()[0].refusal, Some(Refusal::NativeNotListed));
    assert!(!f.acceptable() && f.has_native() && f.entries()[0].enabled);
    assert_eq!(f.decide(true, true, t), vec![Action::Disconnect]);
    // Blocked by the setting.
    let mut c = cfg(d.path());
    c.block_native = true;
    let (f, _) = at_prompt(d.path(), vec![info("nat", &n, true)], c);
    assert_eq!(f.entries()[0].refusal, Some(Refusal::NativeBlocked));
    // Listed with the same file: allowed once the player confirms the red warning.
    let idx = format!(
        r#"{{"schema":1,"generated":"x","mods":[{{"id":"nat","name":"N","versions":[{{"version":"1.0.0","url":"https://x/n.bhmod","sha256":"{}","api":"^0.2"}}]}}]}}"#,
        hex(&sha(&n)).to_uppercase()
    );
    let mut c = cfg(d.path());
    c.index = Some(parse_index(idx.as_bytes()).unwrap());
    let (mut f, t) = at_prompt(d.path(), vec![info("nat", &n, true)], c.clone());
    assert!(f.entries()[0].listed && f.acceptable() && f.has_native());
    assert_eq!(f.decide(true, false, t), vec![Action::Disconnect]);
    let (mut f, t) = at_prompt(d.path(), vec![info("nat", &n, true)], c);
    assert!(matches!(&sent(&f.decide(true, true, t))[0], Message::FileRequest(_)));
    let a = f.packet(&chunks(&n)[0], t);
    let Some(Action::Run(run)) = a.last() else { panic!("{a:?}") };
    assert_eq!(run[0].id, "nat");
}

#[test]
fn reject_text_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    assert_eq!(f.player_info(), PlayerInfo::Hold);
    let r = RejectText { reason_code: 3, text: "Update your mod loader".into(), url: None };
    // The text first, then the game's 0x1f goes out plain so the server's 0x26 follows.
    assert_eq!(
        f.packet(&Message::RejectText(r.clone()).encode(), t),
        vec![Action::Rejected(r.clone()), Action::Release(None)]
    );
    assert_eq!(f.reject(), Some(&r));
    assert_eq!(f.stage(), Stage::Rejected);
}

#[test]
fn entry_names() {
    let mut e = Entry {
        info: info("x", b"", false),
        cached: true,
        listed: false,
        refusal: None,
        have: 0,
        done: true,
        received: None,
        enabled: true,
        known: false,
    };
    assert_eq!(e.to_download(), 0);
    e.cached = false;
    e.info.size = 5;
    assert_eq!(e.to_download(), 5);
    e.info.name = [("tr".to_string(), "Kip".to_string())].into();
    assert_eq!(e.name(), "Kip");
    e.info.name.clear();
    assert_eq!(e.name(), "x");
}

#[test]
fn the_join_packet_gets_the_key() {
    let p = b"\x1f<?xml?><plist><dict><key>alias</key><string>A</string><key>d</key><dict></dict></dict></plist>";
    let out = inject_mod_loader(p, "ab").unwrap();
    let s = String::from_utf8(out[1..].to_vec()).unwrap();
    assert!(s.ends_with("<key>bhcModLoader</key><dict><key>acceptedHash</key><string>ab</string><key>proto</key><integer>1</integer></dict></dict></plist>"), "{s}");
    assert_eq!(out[0], 0x1f);
    assert_eq!(inject_mod_loader(b"\x23<dict></dict>", "ab"), None);
    assert_eq!(inject_mod_loader(b"\x1fbplist00\xff", "ab"), None);
    assert_eq!(inject_mod_loader(b"\x1f<plist/>", "ab"), None);
}

#[test]
fn try_again_restarts_the_download_on_the_same_connection() {
    let d = tempfile::tempdir().unwrap();
    let big = pkg("big", "1.0.0", "return {}\n", "[permissions]\nchannels = true", 2 * CHUNK_SIZE + 10);
    let opt = pkg("opt", "1.0.0", "return {}\n", "", 0);
    let (mut f, t) =
        at_prompt(d.path(), vec![info("big", &big, false), optional(info("opt", &opt, false))], cfg(d.path()));
    // Nothing to retry before a failure, and closing a join that did not fail changes nothing.
    assert!(!f.retryable() && f.retry(t).is_empty() && !f.close());
    let mut f = {
        let (mut g, _) =
            at_prompt(d.path(), vec![info("big", &big, false), optional(info("opt", &opt, false))], cfg(d.path()));
        g.toggle(1);
        std::mem::swap(&mut f, &mut g);
        f
    };
    assert!(!f.entries()[1].enabled);
    f.decide(true, false, t);
    let big_chunks = chunks(&big);
    f.packet(&big_chunks[0], t);
    // The server goes quiet: the download fails and may be retried.
    let late = t + STALL + Duration::from_secs(1);
    assert_eq!(f.tick(late), vec![Action::Disconnect]);
    assert!(f.retryable());
    assert!(matches!(f.stage(), Stage::Failed(Failure::Stalled(_))));
    // Try again: HELLO again, then the list; the earlier choice is replayed (no second prompt) and the partial file
    // resumes after its whole chunks.
    let a = f.retry(late);
    assert!(matches!(sent(&a)[0], Message::Hello(_)));
    assert_eq!(f.stage(), Stage::Downloading);
    assert!(!f.retryable());
    assert_eq!(sent(&f.packet(&hello(2, true), late)), vec![Message::ListRequest]);
    let a = f.packet(&list(vec![info("big", &big, false), optional(info("opt", &opt, false))]), late);
    assert!(!a.contains(&Action::Prompt));
    assert!(!f.entries()[1].enabled);
    assert!(sent(&a).contains(&Message::FileRequest(FileRequest { sha256: sha(&big), offset: CHUNK_SIZE as u64 })));
    let mut run = false;
    for c in &big_chunks[1..] {
        run |= f.packet(c, late).iter().any(|x| matches!(x, Action::Run(m) if m.len() == 1));
    }
    assert!(run);
    assert_eq!(f.stage(), Stage::Joined);
}

#[test]
fn try_again_without_a_server_behind_the_connection_fails_again() {
    let d = tempfile::tempdir().unwrap();
    let big = pkg("big", "1.0.0", "return {}\n", "", 2 * CHUNK_SIZE);
    let (mut f, t) = at_prompt(d.path(), vec![info("big", &big, false)], cfg(d.path()));
    f.decide(true, false, t);
    let late = t + STALL + Duration::from_secs(1);
    f.tick(late);
    f.retry(late);
    // No SERVER_HELLO: not a vanilla server, the connection is gone; the held 0x1f is not released.
    assert!(f.tick(late + Duration::from_millis(1499)).is_empty());
    assert_eq!(f.tick(late + Duration::from_millis(1500)), vec![Action::Disconnect]);
    assert!(matches!(f.stage(), Stage::Failed(Failure::Stalled(ref id)) if id == "big"));
    assert_eq!(f.player_info(), PlayerInfo::Send(None));
    // It can be tried once more; this time the list never comes.
    let a = f.retry(late);
    assert!(matches!(sent(&a)[0], Message::Hello(_)));
    f.packet(&hello(1, true), late);
    let end = late + Duration::from_secs(11);
    assert_eq!(f.tick(end), vec![Action::Disconnect]);
    assert!(matches!(f.stage(), Stage::Failed(Failure::Protocol(_))));
    assert!(!f.retryable());
}

#[test]
fn leaving_the_failure_notice_closes_the_connection_once() {
    let d = tempfile::tempdir().unwrap();
    let big = pkg("big", "1.0.0", "return {}\n", "", 2 * CHUNK_SIZE);
    let (mut f, t) = at_prompt(d.path(), vec![info("big", &big, false)], cfg(d.path()));
    f.decide(true, false, t);
    f.tick(t + STALL + Duration::from_secs(1));
    assert!(f.retryable());
    assert!(f.close());
    assert!(!f.retryable() && !f.close());
    assert!(f.retry(t).is_empty());
}

#[test]
fn the_hash_of_a_received_file_is_taken_before_the_check_deletes_it() {
    let d = tempfile::tempdir().unwrap();
    let small = pkg("small", "1.0.0", "return {}\n", "", CHUNK_SIZE + 500);
    let mut wrong = info("small", &small, false);
    wrong.sha256 = [9; 32];
    // The first chunk is on disk from an earlier try: the download resumes after it.
    let blobs = d.path().join("mods-cache/blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(format!("{}.part", hex(&[9; 32]))), &small[..CHUNK_SIZE]).unwrap();
    let (mut f, t) = at_prompt(d.path(), vec![wrong], cfg(d.path()));
    f.decide(true, false, t);
    assert_eq!(f.entries()[0].received, None);
    // A chunk of a file nobody listed is not recorded.
    let stray = Message::FileChunk(FileChunk { sha256: [1; 32], offset: 0, total: 3, data: vec![1, 2, 3] }).encode();
    f.packet(&stray, t);
    assert_eq!(f.entries()[0].received, None);
    // The last chunk completes the file, which does not match the listed hash.
    let last = Message::FileChunk(FileChunk {
        sha256: [9; 32],
        offset: CHUNK_SIZE as u64,
        total: small.len() as u64,
        data: small[CHUNK_SIZE..].to_vec(),
    });
    f.packet(&last.encode(), t);
    assert_eq!(f.entries()[0].received, Some(hex(&sha(&small))));
    assert!(matches!(f.stage(), Stage::Failed(Failure::HashMismatch(_))));
}

#[test]
fn the_player_count_is_shown_only_when_the_server_gave_one() {
    let d = tempfile::tempdir().unwrap();
    let mut f = JoinFlow::new(cfg(d.path()));
    assert_eq!(f.players(), None);
    f.set_players(3, 16);
    assert_eq!(f.players(), Some((3, 16)));
}

#[test]
fn the_server_hello_player_count_reaches_the_dialog() {
    let d = tempfile::tempdir().unwrap();
    let mut f = JoinFlow::new(cfg(d.path()));
    let t = Instant::now();
    f.contact(t);
    let pkt = Message::ServerHello(ServerHello {
        proto: 1,
        mods_hash: [7; 32],
        client_required: true,
        count: 1,
        players: Some(bhmod_proto::PlayerCount { online: 2, capacity: 8 }),
        features: vec![],
        voice_udp_port: None,
    })
    .encode();
    f.packet(&pkt, t);
    assert_eq!(f.players(), Some((2, 8)));
    // A count set by hand wins over the server's.
    f.set_players(5, 6);
    assert_eq!(f.players(), Some((5, 6)));
}

#[test]
fn voice_and_settings_ops_once_both_sides_listed_them() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    // A server without the features: nothing flows.
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    f.packet(&hello(0, false), t);
    assert!(!f.voice_enabled() && !f.settings_enabled());
    assert_eq!(f.voice_packet(vec![1]), None);
    assert_eq!(f.settings_set_packet("m", "k", vec![0xf5]), None);
    // One that lists them.
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    let h = ServerHello {
        proto: 1,
        mods_hash: [7; 32],
        client_required: false,
        count: 0,
        players: None,
        features: vec!["voice".into(), "settings".into()],
        voice_udp_port: Some(15152),
    };
    f.packet(&Message::ServerHello(h).encode(), t);
    assert!(f.voice_enabled() && f.settings_enabled());
    assert_eq!(f.voice_udp_port(), Some(15152));
    assert_eq!(f.voice_packet(vec![1]), Some(vec![1]));
    assert!(f.settings_set_packet("m", "k", vec![0xf5]).is_some());
    let key = Message::VoiceKey(bhmod_proto::VoiceKey { public: [3; 32], peer: Some(1), udp_port: None });
    assert_eq!(f.packet(&key.encode(), t), vec![Action::Voice(key)]);
    let schema = bhmod_proto::SettingsSchema { mod_id: "m".into(), schema: vec![0xa0], values: vec![0xa0] };
    assert_eq!(f.packet(&Message::SettingsSchema(schema.clone()).encode(), t), vec![Action::SettingsSchema(schema)]);
    let r = bhmod_proto::SettingsResult { mod_id: Some("m".into()), key: None, outcome: Ok(()) };
    assert_eq!(f.packet(&Message::SettingsResult(r.clone()).encode(), t), vec![Action::SettingsResult(r)]);
}

#[test]
fn an_optional_mod_for_a_newer_api_is_skipped_with_a_notice() {
    let d = tempfile::tempdir().unwrap();
    // A package asking for a far newer API than this loader has.
    let nv = {
        let toml = "schema = 1\nid = \"nv\"\nversion = \"1.0.0\"\nname = { en = \"Mod nv\" }\nauthors = [\"a\"]\n\
                    license = \"MIT\"\napi = \"^9.0\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\n\
                    path = \"client/main.luau\"\n";
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        w.start_file("mod.toml", o).unwrap();
        std::io::Write::write_all(&mut w, toml.as_bytes()).unwrap();
        w.start_file("client/main.luau", o).unwrap();
        std::io::Write::write_all(&mut w, b"return {}\n").unwrap();
        w.finish().unwrap().into_inner()
    };
    let (mut f, t) = at_prompt(d.path(), vec![optional(info("nv", &nv, false))], cfg(d.path()));
    f.decide(true, false, t);
    let a = f.packet(&chunks(&nv)[0], t);
    let skipped = a.iter().find_map(|x| if let Action::Skipped(s) = x { Some(s.clone()) } else { None });
    let skipped = skipped.unwrap_or_else(|| panic!("{a:?}"));
    assert_eq!(skipped[0].id, "nv");
}
