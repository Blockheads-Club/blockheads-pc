use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

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
        permissions: ModPermissions { world: "none".into(), players: vec![], net_http: vec![], channels: true, native },
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
        dirs: ModsDirs::new(dir),
        index: None,
        block_native: false,
        address: "127.0.0.1:15151".into(),
    }
}

fn hello(count: u64, required: bool) -> Vec<u8> {
    Message::ServerHello(ServerHello { proto: 1, mods_hash: [7; 32], client_required: required, count }).encode()
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
    assert_eq!(f.stage(), &Stage::Prompt);
    (f, now)
}

#[test]
fn vanilla_server_times_out_and_releases_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let mut f = JoinFlow::new(cfg(d.path()));
    assert_eq!(f.player_info(), PlayerInfo::Send(None));
    let t = Instant::now();
    let a = f.contact(t);
    let Message::Hello(h) = &sent(&a)[0] else { panic!() };
    assert_eq!((h.proto, h.cached.len()), (1, 0));
    assert!(f.contact(t).is_empty());
    assert_eq!(f.player_info(), PlayerInfo::Hold);
    assert!(f.tick(t + Duration::from_millis(1499)).is_empty());
    assert_eq!(f.tick(t + Duration::from_millis(1500)), vec![Action::Release(None)]);
    assert_eq!(f.stage(), &Stage::Vanilla);
    assert!(!f.modded());
    // Late or stray packets change nothing; the 0x1f goes out unchanged from now on.
    assert!(f.packet(&hello(0, false), t).is_empty());
    assert!(f.packet(&[0xbc, 0xff], t).is_empty());
    assert_eq!(f.player_info(), PlayerInfo::Send(None));
    assert!(f.tick(t + Duration::from_secs(9)).is_empty());
}

#[test]
fn server_without_mods_joins_with_the_hash() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    f.player_info();
    let a = f.packet(&hello(0, false), t);
    assert_eq!(a, vec![Action::Release(Some(hex(&[7; 32]))), Action::Run(vec![])]);
    assert_eq!(f.stage(), &Stage::Joined);
    assert_eq!(f.player_info(), PlayerInfo::Send(Some(hex(&[7; 32]))));
    assert!(f.modded());
    assert_eq!(f.server().unwrap().count, 0);
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
    assert!(f.acceptable() && !f.has_native());
    assert_eq!(f.entries()[0].name(), "Mod big");
    assert_eq!(f.progress(), (0, (big.len() + small.len()) as u64));
    // Wrong stage for a decision before; the accept starts at offset 32768.
    let a = f.decide(true, false, t);
    assert_eq!(sent(&a), vec![Message::FileRequest(FileRequest { sha256: sha(&big), offset: CHUNK_SIZE as u64 })]);
    assert!(f.decide(true, false, t).is_empty());
    let cs = chunks(&big);
    // A chunk for another file is ignored; one out of order fails.
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
    assert!(f.entries()[0].done);
    let a = f.packet(&chunks(&small)[0], t);
    let msgs = sent(&a);
    assert!(matches!(&msgs[1], Message::Ready(r) if r.accepted_hash == [7; 32] && r.loaded.len() == 2));
    assert!(a.contains(&Action::Release(Some(hex(&[7; 32])))));
    let Some(Action::Run(run)) = a.last() else { panic!("{a:?}") };
    assert_eq!(run.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["big", "small"]);
    assert_eq!(run[0].grant, ["channels"]);
    assert!(run[0].path.is_file());
    assert!(!blobs.join(format!("{}.part", hex(&sha(&big)))).exists());
    assert_eq!(f.stage(), &Stage::Joined);
    // The launcher's cache record.
    let rec = std::fs::read_to_string(d.path().join("mods-cache/servers/127.0.0.1_15151.toml")).unwrap();
    assert!(rec.contains("Mod big"), "{rec}");
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
    assert_eq!(g.cached_mods().len(), 2);
    assert_eq!(g.progress(), (0, 0));
    let a = g.decide(true, false, t);
    assert!(matches!(&sent(&a)[0], Message::Ready(_)));
    // Channels: from MOD_LIST and CHANNEL_REGISTER; unknown ids are dropped.
    assert_eq!(g.channel_id("echo:ping"), Some(3));
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
}

#[test]
fn channels_need_a_finished_join() {
    let d = tempfile::tempdir().unwrap();
    let small = pkg("small", "1.0.0", "", "", 0);
    let (f, t) = at_prompt(d.path(), vec![info("small", &small, false)], cfg(d.path()));
    assert_eq!(f.channel_packet("echo:ping", b""), None);
    let mut f = f;
    let ch = Message::Channel(bhmod_proto::Channel { channel_id: 3, payload: vec![] }).encode();
    assert!(f.packet(&ch, t).is_empty());
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
    let Stage::Failed(e) = f.stage() else { panic!() };
    assert_eq!(e, &JoinError::Stalled { id: "aa".into(), have: CHUNK_SIZE as u64, size: a.len() as u64 });
    assert!(e.to_string().contains("stopped at 32768"));
    // NOT_FOUND fails at once.
    let d2 = tempfile::tempdir().unwrap();
    let (mut g, t) = at_prompt(d2.path(), vec![info("aa", &a, false)], cfg(d2.path()));
    g.decide(true, false, t);
    assert_eq!(g.packet(&err(file_error::NOT_FOUND), t), vec![Action::Disconnect]);
    assert_eq!(g.stage(), &Stage::Failed(JoinError::File { id: "aa".into(), code: 1 }));
    assert!(matches!(g.stage(), Stage::Failed(e) if e.to_string().contains("error 1")));
}

#[test]
fn bad_chunks_checksums_and_package_rules() {
    let d = tempfile::tempdir().unwrap();
    let a = pkg("aa", "1.0.0", "", "", 0);
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    let bad = Message::FileChunk(FileChunk { sha256: sha(&a), offset: 0, total: a.len() as u64 + 5, data: vec![1] });
    assert_eq!(f.packet(&bad.encode(), t), vec![Action::Disconnect]);
    assert!(matches!(f.stage(), Stage::Failed(e @ JoinError::BadChunk { .. }) if e.to_string().contains("broken")));
    // The server lists a hash the bytes do not have.
    let d = tempfile::tempdir().unwrap();
    let mut wrong = info("aa", &a, false);
    wrong.sha256 = [9; 32];
    let (mut f, t) = at_prompt(d.path(), vec![wrong], cfg(d.path()));
    f.decide(true, false, t);
    let c = Message::FileChunk(FileChunk { sha256: [9; 32], offset: 0, total: a.len() as u64, data: a.clone() });
    assert_eq!(f.packet(&c.encode(), t), vec![Action::Disconnect]);
    let Stage::Failed(e @ JoinError::Checksum { .. }) = f.stage() else { panic!("{:?}", f.stage()) };
    assert!(e.to_string().contains(&hex(&sha(&a))));
    // Not a package (right hash, broken zip): refused by the package rules and deleted.
    let d = tempfile::tempdir().unwrap();
    let junk = b"not a zip".to_vec();
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &junk, false)], cfg(d.path()));
    f.decide(true, false, t);
    assert_eq!(f.packet(&chunks(&junk)[0], t).last(), Some(&Action::Disconnect));
    let Stage::Failed(e @ JoinError::Package { .. }) = f.stage() else { panic!() };
    assert!(e.to_string().starts_with("aa: "));
    assert!(std::fs::read_dir(d.path().join("mods-cache/blobs")).unwrap().next().is_none());
    // A valid package listed under another id; and a cached file whose name lies about its hash.
    let d = tempfile::tempdir().unwrap();
    let (mut f, t) = at_prompt(d.path(), vec![info("bb", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    f.packet(&chunks(&a)[0], t);
    assert!(
        matches!(f.stage(), Stage::Failed(JoinError::Package { message, .. }) if message.contains("holds the mod aa")),
        "{:?}",
        f.stage()
    );
    let d = tempfile::tempdir().unwrap();
    let blobs = d.path().join("mods-cache/blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(format!("{}.bhmod", hex(&[5; 32]))), &a).unwrap();
    let mut lie = info("aa", &a, false);
    lie.sha256 = [5; 32];
    let (mut f, t) = at_prompt(d.path(), vec![lie], cfg(d.path()));
    f.decide(true, false, t);
    assert!(matches!(f.stage(), Stage::Failed(JoinError::Package { message, .. }) if message.contains("not 0505")));
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
    assert!(matches!(f.stage(), Stage::Failed(e @ JoinError::Io(_)) if e.to_string().contains("mod cache")));
    // The finished file cannot be stored (a folder in its place).
    let d = tempfile::tempdir().unwrap();
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    f.decide(true, false, t);
    std::fs::create_dir_all(d.path().join(format!("mods-cache/blobs/{}.bhmod", hex(&sha(&a))))).unwrap();
    assert_eq!(f.packet(&chunks(&a)[0], t), vec![Action::Disconnect]);
    assert!(matches!(f.stage(), Stage::Failed(JoinError::Io(_))));
}

#[test]
fn decline_required_and_optional() {
    let d = tempfile::tempdir().unwrap();
    let a = pkg("aa", "1.0.0", "", "", 0);
    let (mut f, t) = at_prompt(d.path(), vec![info("aa", &a, false)], cfg(d.path()));
    assert_eq!(f.decide(false, false, t), vec![Action::Disconnect]);
    assert_eq!(f.stage(), &Stage::Declined);
    assert_eq!(f.player_info(), PlayerInfo::Send(None));
    // Optional client mods only: declining joins without them (still with the hash).
    let t = Instant::now();
    let mut g = JoinFlow::new(cfg(d.path()));
    g.contact(t);
    g.player_info();
    g.packet(&hello(1, false), t);
    let mut opt = info("aa", &a, false);
    opt.client_required = false;
    g.packet(&list(vec![opt]), t);
    let a = g.decide(false, false, t);
    assert_eq!(a, vec![Action::Release(Some(hex(&[7; 32]))), Action::Run(vec![])]);
    // A list with only server mods needs no prompt.
    let mut h = JoinFlow::new(cfg(d.path()));
    h.contact(t);
    h.packet(&hello(1, false), t);
    let mut srv = info("s", b"x", false);
    srv.sides = vec!["server".into()];
    assert_eq!(h.packet(&list(vec![srv]), t), vec![Action::Run(vec![])]);
}

#[test]
fn native_mods_need_the_index_and_a_confirmation() {
    let d = tempfile::tempdir().unwrap();
    let n = pkg("nat", "1.0.0", "", "[permissions]\nnative = true", 0);
    // Not listed: refused (accepting is impossible and counts as declining).
    let (mut f, t) = at_prompt(d.path(), vec![info("nat", &n, true)], cfg(d.path()));
    assert_eq!(f.entries()[0].refusal, Some(Refusal::NativeNotListed));
    assert!(!f.acceptable() && f.has_native());
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
    assert!(f.entries()[0].listed && f.acceptable());
    assert_eq!(f.decide(true, false, t), vec![Action::Disconnect]);
    let (mut f, t) = at_prompt(d.path(), vec![info("nat", &n, true)], c);
    assert!(matches!(&sent(&f.decide(true, true, t))[0], Message::FileRequest(_)));
}

#[test]
fn reject_text_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let mut f = JoinFlow::new(cfg(d.path()));
    f.contact(t);
    let r = RejectText { reason_code: 3, text: "Update your mod loader".into(), url: None };
    assert_eq!(f.packet(&Message::RejectText(r.clone()).encode(), t), vec![Action::Rejected(r.clone())]);
    assert_eq!(f.reject(), Some(&r));
}

#[test]
fn hashes_and_names() {
    assert_eq!(parse_hash(&"ab".repeat(32)), Some([0xab; 32]));
    assert_eq!(parse_hash("ab"), None);
    assert_eq!(parse_hash(&"zz".repeat(32)), None);
    assert_eq!(parse_hash(&format!("é{}", "a".repeat(62))), None);
    let mut e = Entry { info: info("x", b"", false), cached: true, listed: false, refusal: None, have: 0, done: true };
    assert_eq!(e.to_download(), 0);
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
