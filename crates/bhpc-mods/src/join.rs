//! The client side of the 0xBC join (mod-sdk `docs/protocol-0xbc.md`) for the PC game. The protocol itself (HELLO,
//! the 1500 ms vanilla fallback, `MOD_LIST`, downloads with acknowledgements and resume, SHA-256 and package
//! rules, `READY`, channel ids, `REJECT_TEXT`) is mod-sdk's sans-IO `bhmod_client::Session`, shared with the
//! Android loader. [`JoinFlow`] wraps it for the glue in `bhpc-run`: it keeps what the prompt shows ([`Entry`] per
//! client mod, with the index check and the player's per-mod switch), turns the session's outputs into [`Action`]s
//! and records a finished join in the launcher's server cache. Files go to the launcher's server-mod cache
//! (`mods-cache/blobs/<sha256>.bhmod`, partial files as `.part`).
//!
//! Flow: the server's first packet (0x23) → `HELLO` → wait 1500 ms for `SERVER_HELLO` (none: a vanilla server, the
//! held 0x1f goes out unchanged and nothing 0xBC is sent again) → `LIST_REQUEST` → `MOD_LIST` → the player decides
//! (the game's 0x1f is held meanwhile; nothing blocks the game thread): required mods are fixed, optional ones have
//! a switch, and `READY.loaded` carries the chosen subset → downloads → `READY` → the held 0x1f with
//! `bhcModLoader{acceptedHash, proto: 1}`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bhmod_client::cache::sha256;
use bhmod_client::session::{self, Failure, Offer, Options, Output};
use bhmod_client::{Caps, PackageCache, Session};
use bhmod_manifest::manifest::Runtime;
use bhmod_proto::{hex, Hash, Message, ModInfo, RejectText, PROTO};
use bhpc_launcher::mods::cache;
use bhpc_launcher::mods::index::IndexFile;
use bhpc_launcher::mods::install::Candidate;
use bhpc_launcher::mods::ModsDirs;

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
    /// `HELLO.features` ([`crate::voice::features`]: `settings`, `voice`).
    pub features: Vec<String>,
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
    /// SHA-256 (hex) of the file as it arrived, taken from the bytes before the session checked them. `None` while
    /// nothing complete arrived (cached mods, a download that stopped).
    pub received: Option<String>,
    /// The player took this very file (same SHA-256) when last joining this server and it is still cached: the
    /// prompt does not ask about it again.
    pub known: bool,
    /// The player takes this mod. Always true for required mods; an optional mod can be switched off, and one with
    /// a [`Refusal`] cannot be switched on.
    pub enabled: bool,
}

impl Entry {
    /// The English name (else any, else the id).
    pub fn name(&self) -> String {
        self.info.name.get("en").or_else(|| self.info.name.values().next()).cloned().unwrap_or(self.info.id.clone())
    }

    /// Bytes to download (nothing for a mod that is cached or switched off).
    pub fn to_download(&self) -> u64 {
        if self.cached || !self.enabled {
            0
        } else {
            self.info.size
        }
    }

    /// The server needs this mod on the client (it cannot be switched off).
    pub fn required(&self) -> bool {
        self.info.client_required
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
    /// The server answered the `HELLO` with `REJECT_TEXT` (a protocol mismatch).
    Rejected,
    Failed(Failure),
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
    /// Which cosmetics players wear (`COSMETICS`, after the join).
    Cosmetics(bhmod_proto::Cosmetics),
    /// The server calls a service of a client mod (`SERVICE_CALL`).
    ServiceCallIn(bhmod_proto::ServiceCall),
    /// The server answers a call of a client mod (`SERVICE_RESULT`).
    ServiceResultIn(bhmod_proto::ServiceResult),
    /// `VOICE_KEY`, `VOICE_STATE` or `VOICE_FRAME` for the voice client (both sides listed `voice`).
    Voice(Message),
    /// A mod's server settings for an admin (`SETTINGS_SCHEMA`; both sides listed `settings`).
    SettingsSchema(bhmod_proto::SettingsSchema),
    /// The answer to a `SETTINGS_SET`.
    SettingsResult(bhmod_proto::SettingsResult),
    /// Optional server mods left out because this loader is too old for them (once per join, after the load).
    Skipped(Vec<session::SkippedMod>),
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
#[derive(Debug)]
pub struct JoinFlow {
    cfg: JoinConfig,
    session: Session,
    /// Time zero of the session's millisecond clock (the first call that carries a time).
    epoch: Option<Instant>,
    entries: Vec<Entry>,
    required: bool,
    failure: Option<Failure>,
    held: bool,
    /// What the session decided for the 0x1f (`Some(None)`: plain, `Some(Some(hash))`: with the key).
    joined: Option<Option<Hash>>,
    reject: Option<RejectText>,
    server_name: String,
    /// Players online and the server's capacity set by hand ([`JoinFlow::set_players`]); the count the server put into
    /// `SERVER_HELLO` is the fallback.
    players: Option<(u32, u32)>,
    /// What the session needs to start over on the same connection.
    opts: Options,
    caps: Caps,
    /// The player's choice, kept for "Try again": the optional mods taken and the native confirmation.
    choice: Option<(Vec<String>, bool)>,
    /// A retry is under way: the choice is replayed once the server's list arrives again.
    resume: bool,
    /// The retry found no server behind the connection.
    lost: bool,
    /// The player left the failure notice: the connection is closed.
    closed: bool,
    /// When the download began and what was there then (for the time left).
    started: Option<(Instant, u64)>,
}

impl JoinFlow {
    pub fn new(cfg: JoinConfig) -> JoinFlow {
        let runtimes: Vec<&str> = cfg.runtimes.iter().map(String::as_str).collect();
        let mut opts = Options::new(&cfg.loader, &cfg.platform, &cfg.api, &runtimes);
        opts.stall_timeout_ms = STALL.as_millis() as u64;
        opts.busy_retry_ms = BUSY_RETRY.as_millis() as u64;
        // Native packages are vetted here (index listing, "never allow native", the player's confirmation).
        opts.allow_native = true;
        opts.features = cfg.features.clone();
        let caps = Caps {
            loader: semver::Version::parse(&cfg.loader).unwrap_or_else(|_| semver::Version::new(0, 0, 0)),
            runtimes: cfg
                .runtimes
                .iter()
                .filter_map(|r| match r.as_str() {
                    "lua" => Some(Runtime::Lua),
                    "wasm" => Some(Runtime::Wasm),
                    _ => None,
                })
                .collect(),
        };
        let session = Session::new(opts.clone(), PackageCache::new(cfg.dirs.blobs()), caps.clone());
        let server_name = cfg.address.clone();
        JoinFlow {
            cfg,
            session,
            epoch: None,
            entries: Vec::new(),
            required: false,
            failure: None,
            held: false,
            joined: None,
            reject: None,
            server_name,
            players: None,
            opts,
            caps,
            choice: None,
            resume: false,
            lost: false,
            closed: false,
            started: None,
        }
    }

    /// Players online and capacity, when the server told (shown in the address line): what was set with
    /// [`JoinFlow::set_players`], else the optional count of the server's `SERVER_HELLO`.
    pub fn players(&self) -> Option<(u32, u32)> {
        self.players.or_else(|| self.session.players().map(|p| (p.online, p.capacity)))
    }

    /// Overrides the server's player count (an older server's `SERVER_HELLO` carries none; the dialog shows what is
    /// set here before the server's own).
    pub fn set_players(&mut self, online: u32, capacity: u32) {
        self.players = Some((online, capacity));
    }

    /// The failure may be retried on the same connection: a download that stopped or failed its check.
    pub fn retryable(&self) -> bool {
        !self.closed
            && matches!(
                self.stage(),
                Stage::Failed(Failure::Stalled(_) | Failure::Download { .. } | Failure::HashMismatch(_))
            )
    }

    /// The failure notice was left: no retry any more, the caller disconnects. True if the connection was still open.
    pub fn close(&mut self) -> bool {
        let was = self.retryable();
        self.closed = true;
        was
    }

    /// Starts the download again on the same connection ("Try again"): a fresh session says `HELLO` again (the
    /// server answers it until the join is approved), and when its list arrives the player's earlier choice is
    /// applied, so partial files resume. Does nothing unless [`JoinFlow::retryable`].
    pub fn retry(&mut self, now: Instant) -> Vec<Action> {
        if !self.retryable() {
            return Vec::new();
        }
        let ms = self.ms(now);
        self.session = Session::new(self.opts.clone(), PackageCache::new(self.cfg.dirs.blobs()), self.caps.clone());
        self.failure = None;
        self.resume = true;
        self.lost = false;
        let out = self.session.contact(ms);
        self.handle(out)
    }

    /// The content id map of the server's `MOD_LIST` (empty before it or without content).
    pub fn registry(&self) -> &bhmod_content::registry::Wire {
        self.session.registry()
    }

    pub fn stage(&self) -> Stage {
        if self.lost {
            return Stage::Failed(self.failure.clone().unwrap_or(Failure::Stalled(String::new())));
        }
        if self.resume {
            return Stage::Downloading;
        }
        match self.session.state_name() {
            "idle" => Stage::Idle,
            "await-hello" => Stage::Hello,
            "vanilla" => Stage::Vanilla,
            "await-list" => Stage::Listing,
            "prompt" => Stage::Prompt,
            "download" | "loading" => Stage::Downloading,
            "modded" => Stage::Joined,
            _ => match &self.failure {
                Some(Failure::Declined) => Stage::Declined,
                Some(f) => Stage::Failed(f.clone()),
                None => Stage::Rejected,
            },
        }
    }

    /// The client mods of the server's list.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The server's `SERVER_HELLO.clientRequired`.
    pub fn required(&self) -> bool {
        self.required
    }

    /// The server as the player knows it (`host:port`).
    pub fn address(&self) -> &str {
        &self.cfg.address
    }

    /// Seconds left of the download at the pace since it began (`None` before there is a pace to go by).
    pub fn eta_secs(&self, now: Instant) -> Option<u64> {
        let (at, have0) = self.started?;
        let (have, total) = self.progress();
        let secs = now.saturating_duration_since(at).as_secs_f64();
        let rate = have.saturating_sub(have0) as f64 / secs;
        (secs >= 1.0 && rate > 0.0).then(|| (total.saturating_sub(have) as f64 / rate).ceil() as u64)
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

    /// `(bytes held, bytes to download)` over the mods the player takes.
    pub fn progress(&self) -> (u64, u64) {
        let total = self.entries.iter().map(Entry::to_download).sum();
        let have = self.entries.iter().filter(|e| !e.cached && e.enabled).map(|e| e.have.min(e.info.size)).sum();
        (have, total)
    }

    /// True when the player may accept (no mod they take is refused).
    pub fn acceptable(&self) -> bool {
        self.entries.iter().all(|e| !e.enabled || e.refusal.is_none())
    }

    /// True when a mod the player takes has native code (the prompt needs the red confirmation).
    pub fn has_native(&self) -> bool {
        self.entries.iter().any(|e| e.enabled && e.info.permissions.native)
    }

    /// Switches an optional mod (entry `index`) on or off. Required and refused mods keep their state.
    pub fn toggle(&mut self, index: usize) {
        if self.stage() != Stage::Prompt {
            return;
        }
        if let Some(e) = self.entries.get_mut(index).filter(|e| !e.required() && e.refusal.is_none()) {
            e.enabled = !e.enabled;
        }
    }

    fn ms(&mut self, now: Instant) -> u64 {
        let epoch = *self.epoch.get_or_insert(now);
        now.saturating_duration_since(epoch).as_millis() as u64
    }

    /// The server's first packet (0x23) arrived: send `HELLO`.
    pub fn contact(&mut self, now: Instant) -> Vec<Action> {
        let ms = self.ms(now);
        let out = self.session.contact(ms);
        self.handle(out)
    }

    /// The game is about to send its 0x1f.
    pub fn player_info(&mut self) -> PlayerInfo {
        // The join ended (declined, failed): the connection is closing, the game's 0x1f changes nothing.
        if matches!(self.stage(), Stage::Declined | Stage::Failed(_)) {
            return PlayerInfo::Send(None);
        }
        let out = self.session.join_requested();
        if let Some(Output::Join(j)) = out.first() {
            self.joined = Some(*j);
        }
        match self.joined {
            Some(j) => PlayerInfo::Send(j.map(|h| hex(&h))),
            None => {
                self.held = true;
                PlayerInfo::Hold
            }
        }
    }

    /// Time passes (call it every frame).
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let ms = self.ms(now);
        let out = self.session.tick(ms);
        if self.resume && self.session.state_name() == "vanilla" {
            // Nobody answered the second `HELLO`: the connection is gone. The held 0x1f must not go out as vanilla.
            self.resume = false;
            self.lost = true;
            let id = self.entries.iter().find(|e| !e.done && e.enabled).map(|e| e.info.id.clone()).unwrap_or_default();
            self.failure = Some(Failure::Stalled(id));
            return vec![Action::Disconnect];
        }
        self.handle(out)
    }

    /// A 0xBC packet from the server. Malformed packets are dropped.
    pub fn packet(&mut self, bytes: &[u8], now: Instant) -> Vec<Action> {
        let ms = self.ms(now);
        self.note_received(bytes);
        let out = self.session.receive(bytes, ms);
        self.handle(out)
    }

    /// Voice chat runs on this connection (both sides listed `voice`).
    pub fn voice_enabled(&self) -> bool {
        self.session.voice_enabled()
    }

    /// Server settings can be shown and changed on this connection (both sides listed `settings`).
    pub fn settings_enabled(&self) -> bool {
        self.session.settings_enabled()
    }

    /// The server's dedicated voice UDP port (`SERVER_HELLO.voiceUdpPort`).
    pub fn voice_udp_port(&self) -> Option<u16> {
        self.session.voice_udp_port()
    }

    /// A reliable packet of the voice client (`VOICE_KEY`, `VOICE_CTRL`), `None` unless voice is on.
    pub fn voice_packet(&self, packet: Vec<u8>) -> Option<Vec<u8>> {
        self.session.voice_packet(packet)
    }

    /// The `SETTINGS_SET` packet changing a server setting (`value`: canonical CBOR), `None` unless both sides
    /// listed `settings`.
    pub fn settings_set_packet(&self, mod_id: &str, key: &str, value: Vec<u8>) -> Option<Vec<u8>> {
        self.session.settings_set(mod_id, key, value)
    }

    /// Hashes a file as it arrives: when `bytes` is the last chunk of a listed file, the session is about to check
    /// it and (on a mismatch) delete it, so the hash of what was received is taken here from the part file plus
    /// the chunk.
    fn note_received(&mut self, bytes: &[u8]) {
        let Ok(Message::FileChunk(c)) = Message::decode(bytes) else { return };
        if c.offset + c.data.len() as u64 != c.total {
            return;
        }
        let Some(e) = self.entries.iter_mut().find(|e| e.info.sha256 == c.sha256) else { return };
        let part = self.cfg.dirs.blobs().join(format!("{}.part", hex(&c.sha256)));
        let mut all = std::fs::read(part).unwrap_or_default();
        all.truncate(c.offset as usize);
        all.extend_from_slice(&c.data);
        e.received = Some(hex(&sha256(&all)));
    }

    /// The player's answer to the prompt. `native_confirmed`: the red native warning was confirmed.
    pub fn decide(&mut self, accept: bool, native_confirmed: bool, now: Instant) -> Vec<Action> {
        if self.stage() != Stage::Prompt {
            return Vec::new();
        }
        let ms = self.ms(now);
        let go = accept && self.acceptable() && (native_confirmed || !self.has_native());
        let optional: Vec<String> =
            self.entries.iter().filter(|e| e.enabled && !e.required()).map(|e| e.info.id.clone()).collect();
        if go {
            self.choice = Some((optional.clone(), native_confirmed));
        }
        let out = self.session.decide_subset(go, &optional, ms);
        self.started = Some((now, self.progress().0));
        self.handle(out)
    }

    fn handle(&mut self, outputs: Vec<Output>) -> Vec<Action> {
        let mut actions = Vec::new();
        for o in outputs {
            match o {
                Output::Send(b) => actions.push(Action::Send(b)),
                Output::Prompt { required, mods, .. } => {
                    self.required = required;
                    self.entries = mods.iter().filter(|o| o.client).map(|o| self.entry(o)).collect();
                    self.mark_known();
                    if self.resume {
                        // "Try again": the player already chose; apply it and go on without asking again.
                        self.resume = false;
                        let (optional, native) = self.choice.clone().unwrap_or_default();
                        for e in self.entries.iter_mut().filter(|e| !e.required()) {
                            e.enabled = e.refusal.is_none() && optional.contains(&e.info.id);
                        }
                        actions.extend(self.decide(true, native, Instant::now()));
                        continue;
                    }
                    if !self.entries.is_empty() && self.entries.iter().all(|e| e.known) {
                        // Nothing new since the last join here: take the same mods without asking (their native
                        // code, if any, was confirmed then).
                        actions.extend(self.decide(true, true, Instant::now()));
                        continue;
                    }
                    actions.push(Action::Prompt);
                }
                Output::Progress { id, done, .. } => {
                    if let Some(e) = self.entries.iter_mut().find(|e| e.info.id == id) {
                        e.have = done;
                        e.done = done >= e.info.size;
                    }
                }
                Output::Load(accepted) => {
                    let (next, run) = self.start(accepted);
                    actions.extend(self.handle(next));
                    if let Some(run) = run {
                        actions.push(Action::Run(run));
                    }
                }
                Output::Join(j) => {
                    // Only a held 0x1f waits for this (`player_info` answers the others itself).
                    self.joined = Some(j);
                    self.held = false;
                    actions.push(Action::Release(j.map(|h| hex(&h))));
                }
                Output::Failed(f) => {
                    self.resume = false;
                    self.failure = Some(f);
                    actions.push(Action::Disconnect);
                }
                Output::Reject(r) => {
                    self.reject = Some(r.clone());
                    actions.push(Action::Rejected(r));
                }
                Output::Channel { name, data } => actions.push(Action::ChannelIn(name, data)),
                Output::Cosmetics(c) => actions.push(Action::Cosmetics(c)),
                Output::ServiceCall(c) => actions.push(Action::ServiceCallIn(c)),
                Output::ServiceResult(r) => actions.push(Action::ServiceResultIn(r)),
                Output::Voice(m) => actions.push(Action::Voice(m)),
                Output::SettingsSchema(sc) => actions.push(Action::SettingsSchema(sc)),
                Output::SettingsResult(r) => actions.push(Action::SettingsResult(r)),
                Output::Skipped(m) => actions.push(Action::Skipped(m)),
            }
        }
        actions
    }

    /// Marks the mods taken at the last join of this server (same file, still cached) and takes them again.
    fn mark_known(&mut self) {
        let Some(rec) = cache::server_record(&self.cfg.dirs, &self.cfg.address) else { return };
        for e in &mut self.entries {
            let sha = hex(&e.info.sha256);
            e.known = e.cached && e.refusal.is_none() && rec.mods.iter().any(|m| m.sha256.eq_ignore_ascii_case(&sha));
            e.enabled |= e.known;
        }
    }

    fn entry(&self, o: &Offer) -> Entry {
        let info = o.info.clone();
        let listed = self.cfg.index.as_ref().and_then(|i| i.find(&info.id)).is_some_and(|im| {
            im.versions.iter().any(|v| v.version == info.version && v.sha256.eq_ignore_ascii_case(&hex(&info.sha256)))
        });
        let refusal = match (info.permissions.native, self.cfg.block_native, listed) {
            (false, _, _) => None,
            (true, true, _) => Some(Refusal::NativeBlocked),
            (true, false, false) => Some(Refusal::NativeNotListed),
            (true, false, true) => None,
        };
        Entry {
            have: if o.cached { info.size } else { 0 },
            received: None,
            done: o.cached,
            cached: o.cached,
            listed,
            enabled: info.client_required || refusal.is_none(),
            known: false,
            refusal,
            info,
        }
    }

    /// The session's packages are checked and in the cache: confirm that to the session (which sends `READY`) and
    /// record the join for the launcher. Returns the session's next outputs and the mods to run.
    fn start(&mut self, accepted: Vec<session::Accepted>) -> (Vec<Output>, Option<Vec<Accepted>>) {
        let mut cands = Vec::new();
        let mut run = Vec::new();
        for a in &accepted {
            // The cache names files by hash; make sure the bytes really are that file.
            let bytes = std::fs::read(&a.path).unwrap_or_default();
            let sha = hex(&sha256(&bytes));
            if sha != hex(&a.info.sha256) {
                let _ = std::fs::remove_file(&a.path);
                let why = format!("{}: the cached file is not {}", a.info.id, hex(&a.info.sha256));
                return (self.session.loaded(Err(why)), None);
            }
            let grant = bhpc_launcher::mods::required_permissions(&a.manifest.permissions);
            run.push(Accepted { id: a.info.id.clone(), path: a.path.clone(), grant });
            cands.push(Candidate {
                manifest: a.manifest.clone(),
                bytes,
                sha256: sha,
                source: "server".into(),
                origin: format!("{}-{}.bhmod", a.info.id, a.info.version),
            });
        }
        let loaded = accepted.iter().map(|a| (a.info.id.clone(), a.info.version.clone())).collect();
        let out = self.session.loaded(Ok(loaded));
        // The cache record is informational (the launcher's Server mods tab); a failure does not stop the join.
        let _ = cache::record_join(
            &self.cfg.dirs,
            &self.server_name,
            &self.cfg.address,
            &cands,
            bhpc_launcher::mods::now(),
        );
        (out, Some(run))
    }

    /// The `SERVICE_CALL` packet for a client mod's call, `None` when the join is not modded.
    pub fn service_call_packet(&self, call: &bhmod_core::RemoteCall) -> Option<Vec<u8>> {
        self.session.service_call(bhmod_proto::ServiceCall::from(call))
    }

    /// The `SERVICE_RESULT` packet answering a call of the server, `None` when the join is not modded.
    pub fn service_result_packet(&self, result: bhmod_proto::ServiceResult) -> Option<Vec<u8>> {
        self.session.service_result(result)
    }

    /// The `CHANNEL` packet for a mod's message, `None` when the server has no id for the channel (yet).
    pub fn channel_packet(&self, name: &str, data: &[u8]) -> Option<Vec<u8>> {
        self.session.channel(name, data)
    }
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
