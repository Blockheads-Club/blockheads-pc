//! The client side of the 0xBC join (mod-sdk `docs/protocol-0xbc.md`) as a state machine without I/O on the game:
//! the glue in `bhpc-run` feeds it the server's packets, the game's attempt to send its 0x1f, the player's answer
//! and the time, and carries out the [`Action`]s it returns. Files go to the launcher's server-mod cache
//! (`mods-cache/blobs/<sha256>.bhmod`, partial files as `.part`), and a finished join is recorded with
//! [`bhpc_launcher::mods::cache::record_join`].
//!
//! Flow: the server's first packet (0x23) → `HELLO` → wait 1500 ms for `SERVER_HELLO` (none: a vanilla server, the
//! held 0x1f goes out unchanged and nothing 0xBC is sent again) → `LIST_REQUEST` → `MOD_LIST` → the player decides
//! (the game's 0x1f is held meanwhile; nothing blocks the game thread) → downloads in list order with an
//! acknowledgement every 4 chunks and resume from the `.part` file → SHA-256, package rules, native refusal →
//! `READY` → the held 0x1f with `bhcModLoader{acceptedHash, proto: 1}`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bhmod_proto::{
    hex, Channel, FileRequest, Hash, Hello, Message, ModInfo, ModList, Ready, RejectText, ServerHello, CHUNK_SIZE,
    HELLO_TIMEOUT_MS, PROTO,
};
use bhpc_launcher::mods::cache::{self, CachedMod};
use bhpc_launcher::mods::index::IndexFile;
use bhpc_launcher::mods::install::{self, Candidate};
use bhpc_launcher::mods::ModsDirs;
use sha2::{Digest, Sha256};

/// No chunk for this long: the download failed.
pub const STALL: Duration = Duration::from_secs(20);
/// Wait before asking again after `FILE_ERROR` BUSY.
pub const BUSY_RETRY: Duration = Duration::from_secs(2);

/// Who this loader is and where it keeps files.
#[derive(Debug, Clone)]
pub struct JoinConfig {
    /// `HELLO.loader`.
    pub loader: String,
    /// `HELLO.platform`.
    pub platform: String,
    /// `HELLO.api`.
    pub api: String,
    /// `HELLO.runtimes`.
    pub runtimes: Vec<String>,
    /// The launcher's mod folders (the server-mod cache lives in `cache/blobs`).
    pub dirs: ModsDirs,
    /// The player's copy of the public mod index (for `indexListed`).
    pub index: Option<IndexFile>,
    /// "Never allow native mods".
    pub block_native: bool,
    /// The server as the player knows it (`host:port`), for the cache record.
    pub address: String,
}

/// Why one listed mod cannot be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Native code that the public index does not list with this exact file.
    NativeNotListed,
    /// Native code while "never allow native mods" is on.
    NativeBlocked,
}

/// One client mod of the server's list, as the join prompt shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub info: ModInfo,
    /// Already in the cache (nothing to download).
    pub cached: bool,
    /// The player's index lists this id, version and SHA-256.
    pub listed: bool,
    pub refusal: Option<Refusal>,
    /// Bytes held so far.
    pub have: u64,
    /// Downloaded and checked.
    pub done: bool,
}

impl Entry {
    /// The English name (else any, else the id).
    pub fn name(&self) -> String {
        self.info.name.get("en").or_else(|| self.info.name.values().next()).cloned().unwrap_or(self.info.id.clone())
    }

    /// Bytes to download.
    pub fn to_download(&self) -> u64 {
        if self.cached {
            0
        } else {
            self.info.size
        }
    }
}

/// Why a join failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// The server answered `FILE_ERROR` (not BUSY) for a mod.
    File { id: String, code: u64 },
    /// No data for [`STALL`].
    Stalled { id: String, have: u64, size: u64 },
    /// A chunk out of order or of the wrong size.
    BadChunk { id: String },
    /// The file's SHA-256 is not the listed one.
    Checksum { id: String, expected: String, got: String },
    /// The package rules refused the file.
    Package { id: String, message: String },
    /// The cache cannot be written.
    Io(String),
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::File { id, code } => write!(f, "{id}: the server cannot send the file (error {code})"),
            JoinError::Stalled { id, have, size } => write!(f, "{id}: the download stopped at {have} of {size} bytes"),
            JoinError::BadChunk { id } => write!(f, "{id}: the server sent a broken piece of the file"),
            JoinError::Checksum { id, expected, got } => write!(f, "{id}: expected sha256 {expected}, got {got}"),
            JoinError::Package { id, message } => write!(f, "{id}: {message}"),
            JoinError::Io(m) => write!(f, "cannot write the mod cache: {m}"),
        }
    }
}

/// Where the join is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Not started (no 0x23 yet).
    Idle,
    /// `HELLO` sent; waiting for `SERVER_HELLO`.
    Hello,
    /// No `SERVER_HELLO`: a vanilla server.
    Vanilla,
    /// `LIST_REQUEST` sent.
    Listing,
    /// Waiting for the player.
    Prompt,
    /// Getting files.
    Downloading,
    /// `READY` sent and the 0x1f released.
    Joined,
    /// The player said no to required mods (the connection is closed).
    Declined,
    Failed(JoinError),
}

/// What the glue must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send this packet to the server (reliable).
    Send(Vec<u8>),
    /// Send the held 0x1f now; with `Some(hash)` add `bhcModLoader{acceptedHash: hash, proto: 1}`.
    Release(Option<String>),
    /// Show the join prompt ([`JoinFlow::entries`]).
    Prompt,
    /// Leave the server (declined or failed); the game goes back to its menu.
    Disconnect,
    /// The join is done: run these mods for this server.
    Run(Vec<Accepted>),
    /// The server sent `REJECT_TEXT` (the vanilla 0x26 follows).
    Rejected(RejectText),
    /// A mod channel message from the server: `(channel name, payload)`.
    ChannelIn(String, Vec<u8>),
}

/// A server mod the player accepted, downloaded and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub id: String,
    /// The package in the cache.
    pub path: PathBuf,
    /// Everything it asks for (accepting the prompt grants it).
    pub grant: Vec<String>,
}

/// What the game wants to do with its 0x1f.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerInfo {
    /// Hold it: the flow sends [`Action::Release`] later.
    Hold,
    /// Send it now, with `bhcModLoader` when `Some(hash)`.
    Send(Option<String>),
}

/// One join, from the server's first packet to the game's 0x1f.
#[derive(Debug, Clone)]
pub struct JoinFlow {
    cfg: JoinConfig,
    stage: Stage,
    since: Option<Instant>,
    server: Option<ServerHello>,
    entries: Vec<Entry>,
    /// The file being downloaded (index into `entries`), its bytes, chunks since the last acknowledgement.
    current: Option<(usize, Vec<u8>, u32)>,
    last_data: Option<Instant>,
    retry_at: Option<Instant>,
    held: bool,
    hash: Option<String>,
    channels: BTreeMap<String, u16>,
    reject: Option<RejectText>,
    server_name: String,
}

fn hash_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b).into())
}

impl JoinFlow {
    pub fn new(cfg: JoinConfig) -> JoinFlow {
        let server_name = cfg.address.clone();
        JoinFlow {
            cfg,
            stage: Stage::Idle,
            since: None,
            server: None,
            entries: Vec::new(),
            current: None,
            last_data: None,
            retry_at: None,
            held: false,
            hash: None,
            channels: BTreeMap::new(),
            reject: None,
            server_name,
        }
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The client mods of the server's list.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The server's `SERVER_HELLO`.
    pub fn server(&self) -> Option<&ServerHello> {
        self.server.as_ref()
    }

    /// The server's display name (the address until the game tells a name).
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Sets the display name (the world name from the game).
    pub fn set_server_name(&mut self, name: &str) {
        self.server_name = name.to_string();
    }

    /// The last `REJECT_TEXT`.
    pub fn reject(&self) -> Option<&RejectText> {
        self.reject.as_ref()
    }

    /// True while the server speaks 0xBC on this connection.
    pub fn modded(&self) -> bool {
        self.server.is_some()
    }

    /// Wire id of a channel name (`MOD_LIST.channels`, `CHANNEL_REGISTER`).
    pub fn channel_id(&self, name: &str) -> Option<u16> {
        self.channels.get(name).copied()
    }

    /// `(bytes held, bytes to download)` over every mod to download.
    pub fn progress(&self) -> (u64, u64) {
        let total = self.entries.iter().map(Entry::to_download).sum();
        let have = self.entries.iter().filter(|e| !e.cached).map(|e| e.have.min(e.info.size)).sum();
        (have, total)
    }

    /// True when the player may accept (no mod is refused).
    pub fn acceptable(&self) -> bool {
        self.entries.iter().all(|e| e.refusal.is_none())
    }

    /// True when a listed mod has native code (the prompt needs the red confirmation).
    pub fn has_native(&self) -> bool {
        self.entries.iter().any(|e| e.info.permissions.native)
    }

    fn blob(&self, sha: &Hash, ext: &str) -> PathBuf {
        self.cfg.dirs.cache.join("blobs").join(format!("{}.{ext}", hex(sha)))
    }

    fn cached(&self) -> Vec<Hash> {
        let mut out: Vec<Hash> = std::fs::read_dir(self.cfg.dirs.cache.join("blobs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                parse_hash(name.strip_suffix(".bhmod")?)
            })
            .collect();
        out.sort();
        out
    }

    /// The server's first packet (0x23) arrived: send `HELLO`.
    pub fn contact(&mut self, now: Instant) -> Vec<Action> {
        if self.stage != Stage::Idle {
            return Vec::new();
        }
        self.stage = Stage::Hello;
        self.since = Some(now);
        let hello = Hello {
            proto: PROTO,
            loader: self.cfg.loader.clone(),
            platform: self.cfg.platform.clone(),
            api: self.cfg.api.clone(),
            runtimes: self.cfg.runtimes.clone(),
            cached: self.cached(),
        };
        vec![Action::Send(Message::Hello(hello).encode())]
    }

    /// The game is about to send its 0x1f.
    pub fn player_info(&mut self) -> PlayerInfo {
        match self.stage {
            Stage::Hello | Stage::Listing | Stage::Prompt | Stage::Downloading => {
                self.held = true;
                PlayerInfo::Hold
            }
            Stage::Joined => PlayerInfo::Send(self.hash.clone()),
            _ => PlayerInfo::Send(None),
        }
    }

    /// Releases the held 0x1f (if any).
    fn release(&mut self, hash: Option<String>) -> Vec<Action> {
        if std::mem::take(&mut self.held) {
            vec![Action::Release(hash)]
        } else {
            Vec::new()
        }
    }

    /// Time passes (call it every frame).
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        match &self.stage {
            Stage::Hello if self.since.is_some_and(|s| now >= s + Duration::from_millis(HELLO_TIMEOUT_MS)) => {
                self.stage = Stage::Vanilla;
                self.release(None)
            }
            Stage::Downloading => {
                if self.retry_at.is_some_and(|t| now >= t) {
                    self.retry_at = None;
                    self.last_data = Some(now);
                    return self.request_current();
                }
                if self.retry_at.is_none() && self.last_data.is_some_and(|t| now >= t + STALL) {
                    let (i, have, _) = self.current.as_ref().expect("downloading has a current file");
                    let e = &self.entries[*i];
                    return self.fail(JoinError::Stalled {
                        id: e.info.id.clone(),
                        have: have.len() as u64,
                        size: e.info.size,
                    });
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn fail(&mut self, e: JoinError) -> Vec<Action> {
        self.stage = Stage::Failed(e);
        self.current = None;
        vec![Action::Disconnect]
    }

    /// A 0xBC packet from the server. Malformed packets are dropped.
    pub fn packet(&mut self, bytes: &[u8], now: Instant) -> Vec<Action> {
        let Ok(msg) = Message::decode(bytes) else { return Vec::new() };
        match (msg, &self.stage) {
            (Message::ServerHello(h), Stage::Hello) => self.server_hello(h),
            (Message::ModList(l), Stage::Listing) => self.mod_list(l),
            (Message::FileChunk(c), Stage::Downloading) => self.chunk(c, now),
            (Message::FileError(e), Stage::Downloading) => {
                let (i, _, _) = self.current.as_ref().expect("downloading has a current file");
                if e.sha256 != self.entries[*i].info.sha256 {
                    return Vec::new();
                }
                if e.code == bhmod_proto::file_error::BUSY {
                    self.retry_at = Some(now + BUSY_RETRY);
                    return Vec::new();
                }
                self.fail(JoinError::File { id: self.entries[*i].info.id.clone(), code: e.code })
            }
            (Message::RejectText(r), _) => {
                self.reject = Some(r.clone());
                vec![Action::Rejected(r)]
            }
            (Message::ChannelRegister(c), _) => {
                self.channels.insert(c.name, c.id);
                Vec::new()
            }
            (Message::Channel(Channel { channel_id, payload }), Stage::Joined) => {
                match self.channels.iter().find(|(_, id)| **id == channel_id) {
                    Some((name, _)) => vec![Action::ChannelIn(name.clone(), payload)],
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn server_hello(&mut self, h: ServerHello) -> Vec<Action> {
        let none = h.count == 0;
        self.hash = Some(hex(&h.mods_hash));
        self.server = Some(h);
        if none {
            return self.finish();
        }
        self.stage = Stage::Listing;
        vec![Action::Send(Message::ListRequest.encode())]
    }

    fn mod_list(&mut self, l: ModList) -> Vec<Action> {
        for c in &l.channels {
            self.channels.insert(c.name.clone(), c.id);
        }
        let entries: Vec<Entry> = l
            .mods
            .into_iter()
            .filter(|m| m.sides.iter().any(|s| s == "client"))
            .map(|info| {
                let listed = self.cfg.index.as_ref().and_then(|i| i.find(&info.id)).is_some_and(|im| {
                    im.versions
                        .iter()
                        .any(|v| v.version == info.version && v.sha256.eq_ignore_ascii_case(&hex(&info.sha256)))
                });
                let refusal = match (info.permissions.native, self.cfg.block_native, listed) {
                    (false, _, _) => None,
                    (true, true, _) => Some(Refusal::NativeBlocked),
                    (true, false, false) => Some(Refusal::NativeNotListed),
                    (true, false, true) => None,
                };
                let cached = self.blob(&info.sha256, "bhmod").is_file();
                Entry { have: if cached { info.size } else { 0 }, done: cached, cached, listed, refusal, info }
            })
            .collect();
        self.entries = entries;
        if self.entries.is_empty() {
            return self.finish();
        }
        self.stage = Stage::Prompt;
        vec![Action::Prompt]
    }

    /// The player's answer to the prompt. `native_confirmed`: the red native warning was confirmed.
    pub fn decide(&mut self, accept: bool, native_confirmed: bool, now: Instant) -> Vec<Action> {
        if self.stage != Stage::Prompt {
            return Vec::new();
        }
        let required = self.server.as_ref().is_some_and(|s| s.client_required);
        if !accept || !self.acceptable() || (self.has_native() && !native_confirmed) {
            if required {
                self.stage = Stage::Declined;
                return vec![Action::Disconnect];
            }
            // Only optional client mods: join without them.
            self.entries.clear();
            return self.finish();
        }
        self.stage = Stage::Downloading;
        self.next_download(now)
    }

    /// Starts the next file to download, or finishes.
    fn next_download(&mut self, now: Instant) -> Vec<Action> {
        let Some(i) = self.entries.iter().position(|e| !e.done) else { return self.finish() };
        let mut have = std::fs::read(self.blob(&self.entries[i].info.sha256, "part")).unwrap_or_default();
        have.truncate(have.len() / CHUNK_SIZE * CHUNK_SIZE);
        self.entries[i].have = have.len() as u64;
        self.current = Some((i, have, 0));
        self.last_data = Some(now);
        self.request_current()
    }

    fn request_current(&self) -> Vec<Action> {
        let (i, have, _) = self.current.as_ref().expect("a current file");
        let req = FileRequest { sha256: self.entries[*i].info.sha256, offset: have.len() as u64 };
        vec![Action::Send(Message::FileRequest(req).encode())]
    }

    fn chunk(&mut self, c: bhmod_proto::FileChunk, now: Instant) -> Vec<Action> {
        let (i, mut have, mut since_ack) = self.current.take().expect("downloading has a current file");
        let e = &self.entries[i];
        if c.sha256 != e.info.sha256 {
            self.current = Some((i, have, since_ack));
            return Vec::new();
        }
        if c.offset != have.len() as u64
            || c.total != e.info.size
            || have.len() as u64 + c.data.len() as u64 > e.info.size
        {
            return self.fail(JoinError::BadChunk { id: e.info.id.clone() });
        }
        have.extend_from_slice(&c.data);
        self.last_data = Some(now);
        self.entries[i].have = have.len() as u64;
        let part = self.blob(&self.entries[i].info.sha256, "part");
        if let Err(err) =
            std::fs::create_dir_all(part.parent().expect("blobs dir")).and_then(|_| std::fs::write(&part, &have))
        {
            return self.fail(JoinError::Io(err.to_string()));
        }
        since_ack += 1;
        let size = self.entries[i].info.size;
        if (have.len() as u64) < size {
            let ack = since_ack >= 4;
            let offset = have.len() as u64;
            self.current = Some((i, have, if ack { 0 } else { since_ack }));
            if ack {
                let req = FileRequest { sha256: self.entries[i].info.sha256, offset };
                return vec![Action::Send(Message::FileRequest(req).encode())];
            }
            return Vec::new();
        }
        let e = &self.entries[i];
        let got = hash_hex(&have);
        let expected = hex(&e.info.sha256);
        let _ = std::fs::remove_file(&part);
        if got != expected {
            return self.fail(JoinError::Checksum { id: e.info.id.clone(), expected, got });
        }
        let final_ack = FileRequest { sha256: e.info.sha256, offset: size };
        if let Err(err) = std::fs::write(self.blob(&e.info.sha256, "bhmod"), &have) {
            return self.fail(JoinError::Io(err.to_string()));
        }
        self.entries[i].done = true;
        let mut out = vec![Action::Send(Message::FileRequest(final_ack).encode())];
        out.extend(self.next_download(now));
        out
    }

    /// Every file is present: check them, send `READY`, record the join and release the 0x1f.
    fn finish(&mut self) -> Vec<Action> {
        let server = self.server.clone().expect("finish after SERVER_HELLO");
        let mut cands = Vec::new();
        let mut run = Vec::new();
        for e in &self.entries {
            let path = self.blob(&e.info.sha256, "bhmod");
            // A file that vanished reads as empty and fails the package rules.
            let bytes = std::fs::read(&path).unwrap_or_default();
            let checked = install::inspect(bytes, &format!("{}-{}.bhmod", e.info.id, e.info.version))
                .map_err(|err| err.to_string())
                .and_then(|c| {
                    if c.sha256 != hex(&e.info.sha256) {
                        Err(format!("the cached file is not {}", hex(&e.info.sha256)))
                    } else if c.manifest.id != e.info.id {
                        Err(format!("the file holds the mod {}", c.manifest.id))
                    } else {
                        Ok(c)
                    }
                });
            match checked {
                Ok(c) => {
                    let grant = bhpc_launcher::mods::required_permissions(&c.manifest.permissions);
                    run.push(Accepted { id: e.info.id.clone(), path, grant });
                    cands.push(Candidate { source: "server".into(), ..c });
                }
                Err(message) => {
                    let _ = std::fs::remove_file(&path);
                    return self.fail(JoinError::Package { id: e.info.id.clone(), message });
                }
            }
        }
        self.stage = Stage::Joined;
        let mut out = Vec::new();
        if !self.entries.is_empty() {
            let loaded = self.entries.iter().map(|e| (e.info.id.clone(), e.info.version.clone())).collect();
            out.push(Action::Send(Message::Ready(Ready { accepted_hash: server.mods_hash, loaded }).encode()));
            // The cache record is informational (the launcher's Server mods tab); a failure does not stop the join.
            let _ = cache::record_join(
                &self.cfg.dirs,
                &self.server_name,
                &self.cfg.address,
                &cands,
                bhpc_launcher::mods::now(),
            );
        }
        out.extend(self.release(self.hash.clone()));
        out.push(Action::Run(run));
        out
    }

    /// The `CHANNEL` packet for a mod's message, `None` when the server has no id for the channel (yet).
    pub fn channel_packet(&self, name: &str, data: &[u8]) -> Option<Vec<u8>> {
        if self.stage != Stage::Joined {
            return None;
        }
        let id = self.channel_id(name)?;
        Some(Message::Channel(Channel { channel_id: id, payload: data.to_vec() }).encode())
    }

    /// What the cache records for this server (shown in the prompt as "already downloaded").
    pub fn cached_mods(&self) -> Vec<CachedMod> {
        self.entries
            .iter()
            .filter(|e| e.cached)
            .map(|e| CachedMod {
                id: e.info.id.clone(),
                name: e.name(),
                version: e.info.version.clone(),
                sha256: hex(&e.info.sha256),
                size: e.info.size,
                native: e.info.permissions.native,
            })
            .collect()
    }
}

/// A 64-character hex SHA-256.
pub fn parse_hash(h: &str) -> Option<Hash> {
    if h.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(h.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Adds `bhcModLoader{acceptedHash, proto: 1}` to a 0x1f packet (`0x1f` + an XML property list), before the
/// closing tag of its top-level dict. `None` when the packet is not that (another type, a binary plist).
pub fn inject_mod_loader(packet: &[u8], hash: &str) -> Option<Vec<u8>> {
    let body = packet.strip_prefix(&[0x1f])?;
    let text = std::str::from_utf8(body).ok()?;
    let at = text.rfind("</dict>")?;
    let key = format!(
        "<key>{}</key><dict><key>acceptedHash</key><string>{hash}</string><key>proto</key><integer>{PROTO}</integer></dict>",
        bhmod_proto::PLIST_KEY
    );
    let mut out = vec![0x1f];
    out.extend_from_slice(&text.as_bytes()[..at]);
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(&text.as_bytes()[at..]);
    Some(out)
}
