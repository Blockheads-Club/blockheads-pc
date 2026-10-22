//! The client mod host of one session: [`bhmod_core::ModHost`] (Luau, WASM) with the PC client's
//! [`GameBridge`]. Everything here runs on the game's main thread (the glue only calls it from hooks on that
//! thread and from the frame hook). Bridge calls that need the game (show a chat line, send a channel packet)
//! become [`Effect`]s the glue carries out after the call; mod log lines and per-mod timings are kept for the
//! Mods panel and the launcher.
//!
//! Client API events available on PC now: `chat` (lines the server sends; cancel hides the line, the text can be
//! changed), `command` and mod commands (lines the player types that start with `/`: a mod command runs on the
//! client and is not sent), `tick` (every world update), `player-join` / `player-quit` (the player list the server
//! sends), `world-loaded`, and channels to and from the server. Deferred (no client hook yet): `world-save`,
//! `player-pre-login`, block/object/damage/death/move events and `player.pos`.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use bhmod_core::runtime::{CommandId, InstanceLimits, ModFiles, SubscriptionId, TaskId};
use bhmod_core::storage::FileStorage;
use bhmod_core::types::{
    ApiError, ChannelSource, ChannelTarget, CommandOutcome, CommandSender, Event, LogLevel, Outcome, Role, Side,
};
use bhmod_core::{CallError, GameBridge, HostApi, HostOptions, Instance, ModHost, ModState, Runtime};

/// Longest kept log.
pub const MAX_LOG_LINES: usize = 500;

/// Something a mod asked the game to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show a chat line to the player.
    Chat(String),
    /// Send a channel message to the server.
    Channel { name: String, data: Vec<u8> },
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

/// Time used by one mod.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Over the last whole second.
    pub ms_last_second: f64,
    /// Longest single call.
    pub peak_ms: f64,
    /// Calls so far.
    pub calls: u64,
    window: Duration,
    window_start: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct Timing {
    by_mod: RefCell<BTreeMap<String, Usage>>,
}

impl Timing {
    fn add(&self, id: &str, start: Instant, took: Duration) {
        let mut all = self.by_mod.borrow_mut();
        let u = all.entry(id.to_string()).or_default();
        let ws = *u.window_start.get_or_insert(start);
        if start >= ws + Duration::from_secs(1) {
            u.ms_last_second = u.window.as_secs_f64() * 1000.0;
            u.window = Duration::ZERO;
            u.window_start = Some(start);
        }
        u.window += took;
        u.calls += 1;
        u.peak_ms = u.peak_ms.max(took.as_secs_f64() * 1000.0);
    }
}

/// A runtime whose instances report their time to [`Timing`].
pub(crate) struct Timed {
    inner: Box<dyn Runtime>,
    timing: Rc<Timing>,
}

impl Runtime for Timed {
    fn kind(&self) -> bhmod_manifest::manifest::Runtime {
        self.inner.kind()
    }

    fn instantiate(&self, mod_id: &str, files: &ModFiles, limits: InstanceLimits) -> Result<Box<dyn Instance>, String> {
        let inner = self.inner.instantiate(mod_id, files, limits)?;
        Ok(Box::new(TimedInstance { id: mod_id.to_string(), inner, timing: self.timing.clone() }))
    }
}

struct TimedInstance {
    id: String,
    inner: Box<dyn Instance>,
    timing: Rc<Timing>,
}

impl TimedInstance {
    fn timed<R>(&mut self, f: impl FnOnce(&mut dyn Instance) -> R) -> R {
        let start = Instant::now();
        let r = f(self.inner.as_mut());
        self.timing.add(&self.id, start, start.elapsed());
        r
    }
}

impl Instance for TimedInstance {
    fn on_enable(&mut self, host: &mut dyn HostApi) -> Result<(), CallError> {
        self.timed(|i| i.on_enable(host))
    }
    fn on_disable(&mut self, host: &mut dyn HostApi) -> Result<(), CallError> {
        self.timed(|i| i.on_disable(host))
    }
    fn handle_event(&mut self, host: &mut dyn HostApi, sub: SubscriptionId, ev: &Event) -> Result<Outcome, CallError> {
        self.timed(|i| i.handle_event(host, sub, ev))
    }
    fn run_command(
        &mut self,
        host: &mut dyn HostApi,
        id: CommandId,
        sender: &CommandSender,
        label: &str,
        args: &[String],
    ) -> Result<CommandOutcome, CallError> {
        self.timed(|i| i.run_command(host, id, sender, label, args))
    }
    fn run_task(&mut self, host: &mut dyn HostApi, id: TaskId) -> Result<(), CallError> {
        self.timed(|i| i.run_task(host, id))
    }
    fn call_service(
        &mut self,
        host: &mut dyn HostApi,
        name: &str,
        method: &str,
        args: &[u8],
    ) -> Result<Vec<u8>, CallError> {
        self.timed(|i| i.call_service(host, name, method, args))
    }
    fn on_channel_message(
        &mut self,
        host: &mut dyn HostApi,
        channel: bhmod_core::channels::ChannelId,
        source: &ChannelSource,
        data: &[u8],
    ) -> Result<(), CallError> {
        self.timed(|i| i.on_channel_message(host, channel, source, data))
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
    effects: RefCell<Vec<Effect>>,
    log: RefCell<VecDeque<LogLine>>,
    fresh: RefCell<Vec<LogLine>>,
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
        self.players.borrow().clone()
    }
    fn player_alias(&self, id: &str) -> Option<String> {
        self.players.borrow().iter().find(|p| p.as_str() == id).cloned()
    }
    fn player_role(&self, _id: &str) -> Role {
        Role::Player
    }
    fn send_chat(&self, id: &str, text: &str) -> bool {
        let me = *self.local.borrow() == id;
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

/// The running client mods of one session.
pub struct ClientHost {
    host: ModHost,
    bridge: Rc<Bridge>,
    timing: Rc<Timing>,
    last_tick: Option<Instant>,
}

impl ClientHost {
    /// Loads the session folder (nothing runs before [`ClientHost::enable`]).
    pub fn load(setup: HostSetup<'_>) -> ClientHost {
        let timing = Rc::new(Timing::default());
        let bridge = Rc::new(Bridge {
            started: Instant::now(),
            local: RefCell::new(setup.local_player),
            players: RefCell::new(Vec::new()),
            online: setup.online,
            effects: RefCell::new(Vec::new()),
            log: RefCell::new(VecDeque::new()),
            fresh: RefCell::new(Vec::new()),
        });
        let mut opts = HostOptions::new(setup.mods_dir, Side::Client);
        opts.runtimes = setup
            .runtimes
            .into_iter()
            .map(|inner| Box::new(Timed { inner, timing: timing.clone() }) as Box<dyn Runtime>)
            .collect();
        opts.storage = Box::new(FileStorage::new(setup.storage_dir));
        let host = ModHost::load(opts, bridge.clone());
        ClientHost { host, bridge, timing, last_tick: None }
    }

    /// No mod loaded.
    pub fn is_empty(&self) -> bool {
        self.host.is_empty()
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

    /// A channel message from the server.
    pub fn channel_in(&mut self, name: &str, data: Vec<u8>) -> bool {
        self.host.channel_receive(name, ChannelSource::Server, data)
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

    /// Every loaded mod.
    pub fn status(&self) -> Vec<ModStatus> {
        let timing = self.timing.by_mod.borrow();
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
                let usage = timing.get(&m.id).cloned().unwrap_or_default();
                ModStatus { id: m.id, version: m.version, state, reason, usage }
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

#[cfg(test)]
pub(crate) fn timed_for_tests(inner: Box<dyn Runtime>) -> (Timed, Rc<Timing>) {
    let timing = Rc::new(Timing::default());
    (Timed { inner, timing: timing.clone() }, timing)
}

#[cfg(test)]
pub(crate) fn usage_for_tests(t: &Timing, id: &str) -> Usage {
    t.by_mod.borrow()[id].clone()
}

#[cfg(test)]
pub(crate) fn add_for_tests(t: &Timing, id: &str, start: Instant, took: Duration) {
    t.add(id, start, took)
}
