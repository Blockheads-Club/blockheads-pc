//! The client mod host of one session: [`bhmod_core::ModHost`] (Luau, WASM) with the PC client's
//! [`GameBridge`]. Everything here runs on the game's main thread (the glue only calls it from hooks on that
//! thread and from the frame hook). Bridge calls that need the game (show a chat line, send a channel packet)
//! become [`Effect`]s the glue carries out after the call; mod log lines and per-mod timings are kept for the
//! Mods panel and the launcher.
//!
//! Client API events available on PC now: `chat` (lines the server sends; cancel hides the line, the text can be
//! changed), `command` and mod commands (lines the player types that start with `/`: a mod command runs on the
//! client and is not sent), `tick` (every world update), `player-join` / `player-quit` (the player list the server
//! sends), `world-loaded`, `world-save`, `block-place`, `block-break`, `block-changed`, `object-place`, `damage`,
//! `death`, `player-move`, `player:pos()` and channels to and from the server. The world events are
//! notifications: the server decides what happens, so a mod's cancel or changed amount has no effect on the
//! client. Not on a client: `player-pre-login` (server only).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use bhmod_core::input::InputCommand;
use bhmod_core::remote::{RemoteCall, RemoteError, RemoteTarget};
use bhmod_core::storage::FileStorage;
use bhmod_core::timings::ModTimings;
use bhmod_core::types::{
    ApiError, BreakLayer, ChannelSource, ChannelTarget, CommandSender, Event, LogLevel, ObjectKind, Pos, Role,
    SavePhase, Side,
};
use bhmod_core::ui::{OverheadIcon, UiCommand};
use bhmod_core::voice::VoiceEvent;
use bhmod_core::{GameBridge, HostOptions, ModHost, ModState, ReloadOutcome, Runtime, VoiceHost};

use crate::binds::{BindRow, Binds};
use crate::overhead::Overheads;
use crate::voice::VoiceDriver;
use bhmod_proto::{ServiceCall, ServiceFailure, ServiceResult};

/// Longest kept log.
pub const MAX_LOG_LINES: usize = 500;

/// Most tile types remembered for `block-break` (the table starts over when it is full).
const MAX_TILES: usize = if cfg!(test) { 64 } else { 1 << 16 };

/// Something a mod asked the game to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show a chat line to the player.
    Chat(String),
    /// Send a channel message to the server.
    Channel { name: String, data: Vec<u8> },
    /// Send a service call to the server (`SERVICE_CALL`); the answer comes back through
    /// [`ClientHost::service_result_in`], and a call that cannot be sent is failed with [`ClientHost::service_unavailable`].
    Service(RemoteCall),
    /// A mod asked for the loader's settings screen of a mod (API 0.3.4 `ui.open-settings`).
    OpenSettings(String),
}

/// One log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Milliseconds since the session started.
    pub ms: u64,
    pub level: &'static str,
    pub mod_id: Option<String>,
    pub text: String,
}

fn level_name(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// Time used by one mod, summed up from the host's time accounting ([`bhmod_core::timings`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Mod time per second, averaged over the accounting window (60 s).
    pub ms_last_second: f64,
    /// Longest single call in the window.
    pub peak_ms: f64,
    /// Calls in the window.
    pub calls: u64,
}

impl Usage {
    fn of(t: &ModTimings) -> Usage {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        Usage {
            ms_last_second: ms(t.total()) / t.window.as_secs_f64().max(1.0),
            peak_ms: ms(t.kinds.iter().map(|k| k.max).max().unwrap_or_default()),
            calls: t.calls(),
        }
    }
}

/// The PC client's [`GameBridge`].
pub(crate) struct Bridge {
    started: Instant,
    /// The local player's alias (also their id on the client).
    local: RefCell<String>,
    /// Players in the game, as the server lists them (aliases).
    players: RefCell<Vec<String>>,
    /// Connected to a server (channels to the server work).
    online: bool,
    /// Where each player's blockhead stands (`player:pos()`), as the game last said.
    positions: RefCell<HashMap<String, Pos>>,
    /// The tile types the game filled in, by position (a `block-break` reports what was there).
    tiles: RefCell<HashMap<(i32, i32), u32>>,
    effects: RefCell<Vec<Effect>>,
    log: RefCell<VecDeque<LogLine>>,
    fresh: RefCell<Vec<LogLine>>,
    /// The voice client (API 0.3.4 `voice`), when this session runs voice chat.
    voice: Option<Rc<dyn VoiceDriver>>,
    /// The local player's id on the server (`-[BHNetClientMatch localPlayerID]`), when online.
    local_id: RefCell<Option<String>>,
    /// The mods' key bindings (API 0.3.4 `input`).
    binds: RefCell<Binds>,
    /// The mods' overhead icons (API 0.3.4 `ui.set-overhead`).
    overheads: RefCell<Overheads>,
}

impl Bridge {
    fn push_log(&self, mod_id: Option<&str>, level: LogLevel, text: &str) {
        let line = LogLine {
            ms: self.started.elapsed().as_millis() as u64,
            level: level_name(level),
            mod_id: mod_id.map(str::to_string),
            text: text.to_string(),
        };
        let mut log = self.log.borrow_mut();
        if log.len() == MAX_LOG_LINES {
            log.pop_front();
        }
        log.push_back(line.clone());
        self.fresh.borrow_mut().push(line);
    }
}

impl GameBridge for Bridge {
    fn side(&self) -> Side {
        Side::Client
    }
    fn host_name(&self) -> String {
        format!("blockheads-pc {}", env!("CARGO_PKG_VERSION"))
    }
    fn log(&self, mod_id: Option<&str>, level: LogLevel, message: &str) {
        self.push_log(mod_id, level, message);
    }
    fn online_players(&self) -> Vec<String> {
        // With voice chat the server lists players by id (`VOICE_STATE`), as its mods name them.
        let voiced: Vec<String> =
            self.voice.as_ref().map(|v| v.players()).unwrap_or_default().into_iter().map(|p| p.0).collect();
        let (mut all, me) = if voiced.is_empty() {
            (self.players.borrow().clone(), self.local.borrow().clone())
        } else {
            (voiced, self.local_player().unwrap_or_default())
        };
        // The player is in their own game, in single-player and before the server's list arrives.
        if !all.contains(&me) && !me.is_empty() {
            all.insert(0, me);
        }
        all
    }
    fn player_alias(&self, id: &str) -> Option<String> {
        if let Some(a) = self.voice.as_ref().and_then(|v| v.alias(id)) {
            return Some(a);
        }
        if self.local_id.borrow().as_deref() == Some(id) {
            return Some(self.local.borrow().clone());
        }
        self.online_players().into_iter().find(|p| p.as_str() == id)
    }
    fn local_player(&self) -> Option<String> {
        let me = self.local.borrow();
        self.local_id.borrow().clone().or_else(|| (!me.is_empty()).then(|| me.clone()))
    }
    fn platform(&self) -> String {
        crate::platform()
    }
    fn player_pos(&self, id: &str) -> Option<Pos> {
        self.positions.borrow().get(id).copied()
    }
    fn player_role(&self, _id: &str) -> Role {
        Role::Player
    }
    fn send_chat(&self, id: &str, text: &str) -> bool {
        let me = *self.local.borrow() == id || self.local_id.borrow().as_deref() == Some(id);
        if me {
            self.effects.borrow_mut().push(Effect::Chat(text.to_string()));
        }
        me
    }
    fn broadcast(&self, text: &str) {
        self.effects.borrow_mut().push(Effect::Chat(text.to_string()));
    }
    fn kick(&self, _id: &str, _reason: &str) -> bool {
        false
    }
    fn is_builtin_command(&self, _name: &str) -> bool {
        false
    }
    fn service_send(&self, call: &RemoteCall) -> Result<(), RemoteError> {
        if !self.online {
            return Err(RemoteError::Unavailable);
        }
        self.effects.borrow_mut().push(Effect::Service(call.clone()));
        Ok(())
    }
    fn voice(&self) -> Option<&dyn VoiceHost> {
        self.voice.as_deref().map(VoiceDriver::as_host)
    }
    fn input(&self, mod_id: &str, command: &InputCommand) -> Result<(), ApiError> {
        let mut b = self.binds.borrow_mut();
        match command {
            InputCommand::Bind { action, default_key } => b.bind(mod_id, action, default_key),
            InputCommand::Unbind { action } => {
                b.unbind(mod_id, action);
            }
            InputCommand::ClearMod => b.clear_mod(mod_id),
        }
        Ok(())
    }
    fn input_key(&self, mod_id: &str, action: &str, _default_key: &str) -> Option<String> {
        self.binds.borrow().key_of(mod_id, action)
    }
    fn ui(&self, mod_id: &str, command: &UiCommand) -> Result<(), ApiError> {
        let mut o = self.overheads.borrow_mut();
        match command {
            UiCommand::SetOverhead { player, icon } => o.set(mod_id, player, *icon),
            UiCommand::ClearOverhead { player } => {
                o.clear(mod_id, player);
            }
            UiCommand::OpenSettings { mod_id: of } => self.effects.borrow_mut().push(Effect::OpenSettings(of.clone())),
            UiCommand::ClearMod => o.clear_mod(mod_id),
            // HUD widgets, toasts, dialogs and on-screen buttons are not drawn on PC yet (buttons are a touch control;
            // a desktop mod binds a key instead).
            _ => return Err(ApiError::Unsupported),
        }
        Ok(())
    }
    fn channel_send(&self, _mod_id: &str, name: &str, target: &ChannelTarget, data: &[u8]) -> Result<(), ApiError> {
        match target {
            ChannelTarget::Server if self.online => {
                self.effects.borrow_mut().push(Effect::Channel { name: name.to_string(), data: data.to_vec() });
                Ok(())
            }
            ChannelTarget::Server => Err(ApiError::Unsupported),
            _ => Err(ApiError::InvalidArgument("a client mod sends to the server".into())),
        }
    }
}

/// A mod as the Mods panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModStatus {
    pub id: String,
    pub version: String,
    /// `running`, `loaded`, `budget` (turned off after repeated faults or timeouts) or `failed`.
    pub state: &'static str,
    pub reason: Option<String>,
    pub usage: Usage,
    /// Calls, times, budget hits and strikes per handler kind over the window.
    pub timings: ModTimings,
}

/// Where a session runs.
pub struct HostSetup<'a> {
    /// The session folder ([`crate::session::build`]).
    pub mods_dir: &'a Path,
    /// Mod storage (per server, or `local` for single-player).
    pub storage_dir: &'a Path,
    /// Language runtimes (Luau and WASM by default, see [`default_runtimes`]).
    pub runtimes: Vec<Box<dyn Runtime>>,
    /// Connected to a server.
    pub online: bool,
    pub local_player: String,
    /// The voice client, when the server runs voice chat with this player.
    pub voice: Option<Rc<dyn VoiceDriver>>,
    /// Where the player's keys for mod actions are kept ([`Binds::path`]); `None`: not kept.
    pub binds: Option<std::path::PathBuf>,
    /// The device's file of `scope = "client"` settings ([`client_settings_path`]); `None`: defaults only.
    pub client_settings: Option<std::path::PathBuf>,
}

/// The device's file of the mods' `scope = "client"` settings, in the launcher's data folder.
pub fn client_settings_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("mod-settings.toml")
}

/// Luau and (when the engine starts) WASM, compiling into `cache` for later sessions.
pub fn default_runtimes(cache: &Path) -> Vec<Box<dyn Runtime>> {
    let mut v = bhmod_core::default_runtimes();
    let wasm = bhmod_wasm::WasmConfig { cache_dir: Some(cache.to_path_buf()), ..Default::default() };
    if let Ok(rt) = bhmod_wasm::WasmRuntime::new(wasm) {
        v.push(Box::new(rt));
    }
    v
}

/// True when this build has the JVM runtime (feature `jvm`).
pub const JVM_BUILT: bool = cfg!(feature = "jvm");

/// The runtimes of a session: Luau and WASM, plus the JVM when `jvm` asks for it. `java_home` picks the JRE (`None`
/// searches `BHMOD_JAVA_HOME`, `JAVA_HOME` and `java` on the `PATH`). The note says what happened with the JVM, for
/// the log (`None` when it was not asked for). JVM mods are unsandboxed, so this is opt-in all the way down.
pub fn session_runtimes(cache: &Path, jvm: bool, java_home: Option<&Path>) -> (Vec<Box<dyn Runtime>>, Option<String>) {
    let mut runtimes = default_runtimes(&cache.join("wasm"));
    let note = jvm.then(|| add_jvm(&mut runtimes, java_home, &cache.join("jvm")));
    (runtimes, note)
}

#[cfg(feature = "jvm")]
fn add_jvm(runtimes: &mut Vec<Box<dyn Runtime>>, java_home: Option<&Path>, work_dir: &Path) -> String {
    let cfg = bhmod_core::jvm::JvmConfig {
        java_home: java_home.map(Path::to_path_buf),
        work_dir: Some(work_dir.to_path_buf()),
        ..Default::default()
    };
    match bhmod_core::jvm::JvmRuntime::new(cfg) {
        Ok(rt) => {
            runtimes.push(Box::new(rt));
            "JVM mods are on: they run unsandboxed".to_string()
        }
        Err(e) => format!("JVM mods are on but no JVM started: {e}"),
    }
}

#[cfg(not(feature = "jvm"))]
fn add_jvm(_: &mut Vec<Box<dyn Runtime>>, _: Option<&Path>, _: &Path) -> String {
    "JVM mods were asked for but this build has no JVM support (build with the `jvm` feature)".to_string()
}

/// The running client mods of one session.
pub struct ClientHost {
    host: ModHost,
    bridge: Rc<Bridge>,
    last_tick: Option<Instant>,
}

impl ClientHost {
    /// Loads the session folder (nothing runs before [`ClientHost::enable`]).
    pub fn load(setup: HostSetup<'_>) -> ClientHost {
        let bridge = Rc::new(Bridge {
            started: Instant::now(),
            local: RefCell::new(setup.local_player),
            players: RefCell::new(Vec::new()),
            online: setup.online,
            positions: RefCell::new(HashMap::new()),
            tiles: RefCell::new(HashMap::new()),
            effects: RefCell::new(Vec::new()),
            log: RefCell::new(VecDeque::new()),
            fresh: RefCell::new(Vec::new()),
            voice: setup.voice,
            local_id: RefCell::new(None),
            binds: RefCell::new(setup.binds.as_deref().map(Binds::load).unwrap_or_default()),
            overheads: RefCell::new(Overheads::default()),
        });
        let mut opts = HostOptions::new(setup.mods_dir, Side::Client);
        opts.runtimes = setup.runtimes;
        opts.storage = Box::new(FileStorage::new(setup.storage_dir));
        opts.client_settings = setup.client_settings;
        let host = ModHost::load(opts, bridge.clone());
        ClientHost { host, bridge, last_tick: None }
    }

    /// No mod loaded.
    pub fn is_empty(&self) -> bool {
        self.host.is_empty()
    }

    /// True when a mod listens to a world event (the glue skips reading the game's arguments otherwise).
    pub fn watching(&self) -> bool {
        use bhmod_core::types::EventKind as K;
        [
            K::WorldSave,
            K::BlockPlace,
            K::BlockBreak,
            K::BlockChanged,
            K::ObjectPlace,
            K::Damage,
            K::Death,
            K::PlayerMove,
        ]
        .into_iter()
        .any(|k| self.host.wants(k))
    }

    /// The world is ready: `on-enable` and `world-loaded`.
    pub fn enable(&mut self, world_name: &str, world_id: &str) {
        self.host.enable(world_name, world_id);
    }

    /// Leaving the world: `on-disable` for every mod, storage flushed.
    pub fn stop(&mut self) {
        self.host.disable_all();
    }

    /// The local player's alias.
    pub fn set_local_player(&self, alias: &str) {
        *self.bridge.local.borrow_mut() = alias.to_string();
    }

    /// The local player's id on the server (`players.local-player`; voice chat names players by id).
    pub fn set_local_id(&self, id: Option<String>) {
        *self.bridge.local_id.borrow_mut() = id.filter(|i| !i.is_empty());
    }

    /// The players as the mods name them (`players.online`: aliases, or ids once voice chat lists them).
    pub fn online(&self) -> Vec<String> {
        self.bridge.online_players()
    }

    /// The player clicked a blockhead: `<mod>:ui-pick {player}` for every mod.
    pub fn pick(&mut self, player: &str) {
        self.host.ui_pick(player);
    }

    /// A chat message from the server (`alias` empty for server messages). `None`: a mod hid it; otherwise the
    /// text to show (possibly changed).
    pub fn chat_received(&mut self, alias: &str, text: &str) -> Option<String> {
        if !self.host.wants(bhmod_core::types::EventKind::Chat) {
            return Some(text.to_string());
        }
        let ev = self.host.fire(Event::Chat { player: alias.to_string(), text: text.to_string(), cancelled: false });
        // `fire` hands back the event it was given, with the handlers' changes.
        let Event::Chat { text: new, cancelled, .. } = ev else { return None };
        (!cancelled).then_some(new)
    }

    /// A line the player typed. True when a mod handled it (a mod command, or a cancelled `command` event): it is
    /// then not sent to the server; the replies arrive as [`Effect::Chat`].
    pub fn typed(&mut self, line: &str) -> bool {
        if !line.starts_with('/') {
            return false;
        }
        let me = self.bridge.local.borrow().clone();
        match self.host.command(CommandSender::Player(me), line) {
            Some(replies) => {
                self.bridge.effects.borrow_mut().extend(replies.into_iter().map(Effect::Chat));
                true
            }
            None => false,
        }
    }

    /// One world update.
    pub fn tick(&mut self, now: Instant) {
        let dt = self.last_tick.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        self.last_tick = Some(now);
        self.host.tick(dt);
    }

    /// Once per frame: next-poll tasks and channel deliveries.
    pub fn poll(&mut self) {
        self.host.poll();
    }

    /// The server's player list changed: `player-join` / `player-quit` for the differences.
    pub fn players(&mut self, now: Vec<String>) {
        let before = self.bridge.players.replace(now.clone());
        for p in now.iter().filter(|p| !before.contains(p)) {
            self.host.fire(Event::PlayerJoin { player: p.clone() });
        }
        for p in before.iter().filter(|p| !now.contains(p)) {
            self.host.fire(Event::PlayerQuit { player: p.clone(), was_kick: false });
        }
    }

    /// Delivers a world event to the mods that listen. The server decides what happens in the world, so what the
    /// handlers answer (cancel, a changed amount) is dropped. Events about a player the game does not list are
    /// skipped (a mod cannot hold a handle to them).
    fn watch(&mut self, ev: Event) {
        if self.host.wants(ev.kind()) {
            self.host.fire(ev);
        }
    }

    fn listed(&self, player: &str) -> bool {
        self.bridge.player_alias(player).is_some()
    }

    /// `world-save`: the game saves the world (`before` or after).
    pub fn world_saved(&mut self, before: bool) {
        self.watch(Event::WorldSave { phase: if before { SavePhase::Pre } else { SavePhase::Post } });
    }

    /// A tile of type `tile` was put at `at` by `player` (`block-place`, `block-changed`).
    pub fn block_placed(&mut self, player: &str, at: (i32, i32), tile: u32, data: (u16, u16)) {
        let pos = Pos { x: at.0, y: at.1 };
        {
            let mut tiles = self.bridge.tiles.borrow_mut();
            if tiles.len() >= MAX_TILES {
                tiles.clear();
            }
            tiles.insert(at, tile);
        }
        let known = self.listed(player);
        if known {
            self.watch(Event::BlockPlace {
                player: player.to_string(),
                pos,
                item: tile,
                data_a: data.0,
                data_b: data.1,
                cancelled: false,
            });
        }
        self.watch(Event::BlockChanged { pos, tile, player: known.then(|| player.to_string()) });
    }

    /// The tile or back wall at `at` was removed by `player` (`block-break`, `block-changed`).
    pub fn block_broken(&mut self, player: &str, at: (i32, i32), back_wall: bool) {
        let pos = Pos { x: at.0, y: at.1 };
        let tile = if back_wall { 0 } else { self.bridge.tiles.borrow_mut().remove(&at).unwrap_or(0) };
        let known = self.listed(player);
        if known {
            let layer = if back_wall { BreakLayer::BackWall } else { BreakLayer::Block };
            self.watch(Event::BlockBreak { player: player.to_string(), pos, layer, tile, cancelled: false });
        }
        if !back_wall {
            self.watch(Event::BlockChanged { pos, tile: 0, player: known.then(|| player.to_string()) });
        }
    }

    /// A workbench (`item` is its type) or an interaction object was placed (`object-place`).
    pub fn object_placed(&mut self, player: &str, at: (i32, i32), workbench: bool, item: u32) {
        if self.listed(player) {
            let object = if workbench { ObjectKind::Workbench } else { ObjectKind::InteractionObject };
            let pos = Pos { x: at.0, y: at.1 };
            self.watch(Event::ObjectPlace { player: player.to_string(), pos, object, item, cancelled: false });
        }
    }

    /// A blockhead (`blockhead` is its id) took damage (`damage`); `owner` is the player it belongs to.
    pub fn damaged(&mut self, owner: Option<&str>, blockhead: u64, amount: f32) {
        let player = owner.filter(|p| self.listed(p)).map(str::to_string);
        self.watch(Event::Damage { player, blockhead, amount, attacker: None, cancelled: false });
    }

    /// A blockhead died (`death`).
    pub fn died(&mut self, owner: Option<&str>, blockhead: u64) {
        let player = owner.filter(|p| self.listed(p)).map(str::to_string);
        self.watch(Event::Death { player, blockhead });
    }

    /// A player's blockhead stands on tile `to` now: `player:pos()` follows, `player-move` fires when it changed.
    pub fn moved(&mut self, player: &str, to: (i32, i32)) {
        let to = Pos { x: to.0, y: to.1 };
        let from = self.bridge.positions.borrow_mut().insert(player.to_string(), to);
        if let Some(from) = from.filter(|f| *f != to).filter(|_| self.listed(player)) {
            self.watch(Event::PlayerMove { player: player.to_string(), from, to });
        }
    }

    /// A channel message from the server.
    pub fn channel_in(&mut self, name: &str, data: Vec<u8>) -> bool {
        self.host.channel_receive(name, ChannelSource::Server, data)
    }

    /// The server calls a service of a client mod (`SERVICE_CALL`). The answer is for the glue to send back.
    pub fn service_call_in(&mut self, call: ServiceCall) -> ServiceResult {
        let out = self.host.remote_call_in(
            &RemoteTarget::Server,
            &call.mod_id,
            &call.service,
            &call.version_req,
            &call.method,
            &call.args,
        );
        ServiceResult { id: call.id, outcome: out.map_err(|e| ServiceFailure::from(&e)) }
    }

    /// The server's answer to a call of a client mod (`SERVICE_RESULT`). False when no such call is open.
    pub fn service_result_in(&mut self, r: ServiceResult) -> bool {
        self.host.remote_result(&RemoteTarget::Server, r.id, r.outcome.map_err(RemoteError::from))
    }

    /// A call of a client mod could not be sent (the server has no id for it, or the join is not modded): the
    /// mod hears `unavailable`.
    pub fn service_unavailable(&mut self, id: u32) {
        self.host.remote_result(&RemoteTarget::Server, id, Err(RemoteError::Unavailable));
    }

    /// The connection to the server is gone: open service calls fail with `disconnected`.
    pub fn server_gone(&mut self) {
        self.host.remote_peer_gone(&RemoteTarget::Server);
    }

    /// Voice chat happened (talk start/stop, states, the microphone): `voice:*` events for the mods with `voice`.
    pub fn voice_events(&mut self, events: Vec<VoiceEvent>) {
        for e in &events {
            self.host.voice_event(e);
        }
    }

    /// A key with an `input` name went down or up (not while typing): `<mod>:input` for every action bound to it.
    /// True when some mod's action took it.
    pub fn key(&mut self, name: &str, pressed: bool) -> bool {
        let hits = self.bridge.binds.borrow().find(name);
        for (m, a) in &hits {
            self.host.input_event(m, a, pressed);
        }
        !hits.is_empty()
    }

    /// The key bindings of a mod (settings screen).
    pub fn binds(&self, mod_id: &str) -> Vec<BindRow> {
        self.bridge.binds.borrow().rows(mod_id)
    }

    /// The player changed a key of a mod's action (`None` clears it).
    pub fn rebind(&self, mod_id: &str, action: &str, key: Option<&str>) -> std::io::Result<()> {
        self.bridge.binds.borrow_mut().set_key(mod_id, action, key)
    }

    /// The overhead icon per player, without the ones the admin turned off.
    pub fn overheads(&self) -> Vec<(String, OverheadIcon)> {
        let v = self.bridge.voice.as_ref().and_then(|d| d.as_host().icons().ok()).unwrap_or_default();
        self.bridge.overheads.borrow().shown(&v)
    }

    /// The mod's `scope = "client"` settings form: `({"fields": [...]}, values)`.
    pub fn client_settings(&self, mod_id: &str) -> Option<(serde_json::Value, serde_json::Value)> {
        self.host.settings_form(mod_id, bhmod_manifest::settings::Scope::Client)
    }

    /// The player changes a client setting (stored on the device). The error is the reason for the player.
    pub fn set_client_setting(&mut self, mod_id: &str, key: &str, value: &serde_json::Value) -> Result<(), String> {
        self.host.set_setting(mod_id, key, value).map(|_| ()).map_err(|e| e.to_string())
    }

    /// Effects queued by the mods since the last call.
    pub fn take_effects(&self) -> Vec<Effect> {
        std::mem::take(&mut *self.bridge.effects.borrow_mut())
    }

    /// Log lines since the last call.
    pub fn take_new_logs(&self) -> Vec<LogLine> {
        std::mem::take(&mut *self.bridge.fresh.borrow_mut())
    }

    /// The kept log (newest last).
    pub fn log(&self) -> Vec<LogLine> {
        self.bridge.log.borrow().iter().cloned().collect()
    }

    /// Hot reload (developer mode, F5): the mods whose package in the session folder changed, or just `only`, are
    /// stopped and started again without leaving the world. Nothing else is touched: storage stays, the other mods
    /// keep running. See [`bhmod_core::ModHost::reload`].
    ///
    /// Returns what happened, one line per mod that changed (nothing when no package changed).
    pub fn reload(&mut self, only: Option<&str>) -> Vec<String> {
        let results = self.host.reload(only);
        results.iter().filter(|r| r.outcome != ReloadOutcome::Unchanged).map(ToString::to_string).collect()
    }

    /// Clears the time accounting (all mods, or `only`); false when `only` names no mod.
    pub fn reset_timings(&mut self, only: Option<&str>) -> bool {
        self.host.reset_timings(only)
    }

    /// Every loaded mod.
    pub fn status(&self) -> Vec<ModStatus> {
        let mut timings = self.host.timings(None);
        self.host
            .mods()
            .into_iter()
            .map(|m| {
                let (state, reason) = match m.state {
                    ModState::Enabled => ("running", None),
                    ModState::Loaded => ("loaded", None),
                    ModState::Disabled(r) if r.contains("faults within") => ("budget", Some(r)),
                    ModState::Disabled(r) => ("failed", Some(r)),
                };
                let at = timings.iter().position(|t| t.id == m.id).expect("the host times every loaded mod");
                let timings = timings.swap_remove(at);
                ModStatus { id: m.id, version: m.version, state, reason, usage: Usage::of(&timings), timings }
            })
            .collect()
    }
}

#[cfg(test)]
impl ClientHost {
    pub(crate) fn bridge_for_tests(&self) -> Rc<Bridge> {
        self.bridge.clone()
    }
}

#[cfg(test)]
impl Bridge {
    pub(crate) fn players_for_tests(&self) -> std::cell::RefMut<'_, Vec<String>> {
        self.players.borrow_mut()
    }
}
