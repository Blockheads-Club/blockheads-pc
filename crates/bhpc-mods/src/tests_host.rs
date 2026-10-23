use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bhmod_core::runtime::{InstanceLimits, ModFiles};
use bhmod_core::types::{ApiError, ChannelSource, ChannelTarget, CommandSender, Event, EventKind, Outcome, Priority};
use bhmod_core::{CallError, GameBridge, HostApi, Instance, Runtime};

use crate::host::*;
use crate::join::Accepted;
use crate::session::{build, Placed, Plan};
use crate::tests_join::pkg;

const MAIN: &str = r#"
local ticks = 0
local ch
bh.events.on("chat", function(ev)
    if ev.text == "hide" then ev.cancelled = true elseif ev.text == "up" then ev.text = "UP" end
end)
bh.events.on("tick", function(ev) ticks = ticks + 1 end)
bh.events.on("player-join", function(ev) bh.log.info("join " .. ev.player:alias()) end)
bh.events.on("player-quit", function(ev) bh.log.info("quit " .. ev.player:id()) end)
bh.events.on("command", function(ev) if ev.name == "blocked" then ev.cancelled = true end end)
bh.commands.register({ name = "hello", usage = "/hello" }, function(sender, label, args)
    sender.player:send_chat("to me")
    bh.players.broadcast("everyone")
    return { handled = true, reply = { "hi " .. #args .. " " .. ticks } }
end)
bh.commands.register({ name = "spin" }, function() while true do end end)
return {
    on_enable = function()
        ch = bh.channels.open("echo:ping", function(origin, data)
            bh.channels.send(ch, "server", "pong:" .. data)
        end)
        bh.log.warn("enabled")
    end,
}
"#;

fn echo_pkg() -> Vec<u8> {
    pkg("echo", "1.0.0", MAIN, "[permissions]\nchannels = true\nplayers = [\"chat\"]", 0)
}

fn setup(dir: &Path, online: bool, plan: Plan) -> (ClientHost, Vec<Placed>, Vec<String>) {
    let mods = dir.join("session");
    let (placed, problems) = build(&mods, &plan).unwrap();
    let h = ClientHost::load(HostSetup {
        mods_dir: &mods,
        storage_dir: &dir.join("storage"),
        runtimes: bhmod_core::default_runtimes(),
        online,
        local_player: "ME".into(),
    });
    (h, placed, problems)
}

fn server_plan(dir: &Path) -> Plan {
    let p = dir.join("echo.bhmod");
    std::fs::write(&p, echo_pkg()).unwrap();
    Plan {
        own: None,
        dev: vec![],
        server: vec![Accepted { id: "echo".into(), path: p, grant: vec!["channels".into(), "players:chat".into()] }],
    }
}

#[test]
fn chat_commands_ticks_players_and_channels() {
    let d = tempfile::tempdir().unwrap();
    let (mut h, placed, problems) = setup(d.path(), true, server_plan(d.path()));
    assert_eq!(placed, vec![Placed { id: "echo".into(), origin: "server" }]);
    assert!(problems.is_empty());
    assert!(!h.is_empty());
    // Before enable nothing runs.
    assert_eq!(h.chat_received("A", "hide"), Some("hide".into()));
    assert!(!h.typed("/hello"));
    h.enable("World", "w1");
    h.enable("World", "w1");
    assert!(h
        .take_new_logs()
        .iter()
        .any(|l| l.text == "enabled" && l.level == "warn" && l.mod_id.as_deref() == Some("echo")));
    assert!(h.take_new_logs().is_empty());
    h.set_local_player("ME");
    assert!(!h.watching(), "the echo mod does not listen to the world");
    assert_eq!(h.chat_received("BOB", "hide"), None);
    assert_eq!(h.chat_received("BOB", "up"), Some("UP".into()));
    assert_eq!(h.chat_received("", "up"), Some("UP".into()));
    assert_eq!(h.chat_received("BOB", "same"), Some("same".into()));
    let t = Instant::now();
    h.tick(t);
    h.tick(t + Duration::from_millis(16));
    assert!(!h.typed("hello"));
    assert!(!h.typed("/unknown"));
    assert!(h.typed("/blocked"));
    assert!(h.typed("/hello a b"));
    assert_eq!(
        h.take_effects(),
        vec![Effect::Chat("to me".into()), Effect::Chat("everyone".into()), Effect::Chat("hi 2 2".into())]
    );
    h.players(vec!["ME".into(), "BOB".into()]);
    h.players(vec!["ME".into()]);
    let logs: Vec<String> = h.take_new_logs().into_iter().map(|l| l.text).collect();
    assert!(logs.contains(&"join BOB".to_string()) && logs.contains(&"quit BOB".to_string()), "{logs:?}");
    assert!(h.channel_in("echo:ping", b"x".to_vec()));
    h.poll();
    assert_eq!(h.take_effects(), vec![Effect::Channel { name: "echo:ping".into(), data: b"pong:x".to_vec() }]);
    let st = h.status();
    assert_eq!((st[0].id.as_str(), st[0].state, st[0].usage.calls > 3), ("echo", "running", true));
    assert!(!h.log().is_empty());
    // A runaway command: three timeouts turn the mod off (the budget state).
    for _ in 0..3 {
        assert!(h.typed("/spin"));
    }
    let st = h.status();
    assert_eq!(st[0].state, "budget", "{st:?}");
    assert!(st[0].reason.as_deref().unwrap().contains("faults"));
    assert!(st[0].usage.peak_ms >= 20.0);
    h.stop();
}

#[test]
fn offline_channels_failed_mods_and_the_log_limit() {
    let d = tempfile::tempdir().unwrap();
    // Offline (single-player): sending to the server is unsupported.
    let (mut h, _, _) = setup(d.path(), false, server_plan(d.path()));
    h.enable("W", "w");
    h.channel_in("echo:ping", b"x".to_vec());
    h.poll();
    assert!(h.take_effects().is_empty());
    assert!(h.log().iter().any(|l| l.level == "error" && l.text.contains("unsupported")), "{:?}", h.log());
    // A mod whose on_enable fails is shown as failed; a never-enabled one as loaded.
    let d = tempfile::tempdir().unwrap();
    let own = d.path().join("mods");
    std::fs::create_dir_all(&own).unwrap();
    let bad = "return { on_enable = function() error('boom') end }";
    std::fs::write(own.join("bad.bhmod"), pkg("bad", "1.0.0", bad, "", 0)).unwrap();
    let (mut h, _, _) = setup(d.path(), true, Plan { own: Some(own.clone()), ..Default::default() });
    assert_eq!(h.status()[0].state, "loaded");
    h.enable("W", "w");
    assert_eq!(h.status()[0].state, "failed");
    // The kept log is bounded.
    let b = h.bridge_for_tests();
    for _ in 0..MAX_LOG_LINES + 5 {
        b.log(None, bhmod_core::types::LogLevel::Info, "x");
    }
    assert_eq!(h.log().len(), MAX_LOG_LINES);
}

#[test]
fn bridge_answers() {
    let d = tempfile::tempdir().unwrap();
    let (h, _, _) = setup(d.path(), true, Plan::default());
    assert!(h.is_empty());
    let b = h.bridge_for_tests();
    assert_eq!(b.side(), bhmod_core::types::Side::Client);
    assert!(b.host_name().starts_with("blockheads-pc "));
    *b.players_for_tests() = vec!["ME".into(), "BOB".into()];
    assert_eq!(b.online_players().len(), 2);
    assert_eq!(b.player_alias("BOB"), Some("BOB".into()));
    assert_eq!(b.player_alias("X"), None);
    assert_eq!(b.player_role("BOB"), bhmod_core::types::Role::Player);
    assert!(!b.send_chat("BOB", "x"));
    assert!(!b.kick("BOB", "x"));
    assert!(!b.is_builtin_command("help"));
    assert!(matches!(b.channel_send("m", "c", &ChannelTarget::AllPlayers, b""), Err(ApiError::InvalidArgument(_))));
    for l in [bhmod_core::types::LogLevel::Trace, bhmod_core::types::LogLevel::Debug] {
        b.log(None, l, "x");
    }
    assert_eq!(h.take_new_logs().iter().map(|l| l.level).collect::<Vec<_>>(), ["trace", "debug"]);
}

#[test]
fn default_runtimes_have_luau_and_wasm() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(default_runtimes(d.path()).len(), 2);
}

#[test]
fn session_folder_merges_server_dev_and_own_mods() {
    let d = tempfile::tempdir().unwrap();
    let own = d.path().join("mods");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("echo.bhmod"), pkg("echo", "0.9.0", "", "", 0)).unwrap();
    std::fs::write(own.join("mine.bhmod"), pkg("mine", "1.0.0", "", "[permissions]\nchannels = true", 0)).unwrap();
    std::fs::write(own.join("off.bhmod"), pkg("off", "1.0.0", "", "", 0)).unwrap();
    std::fs::write(own.join("broken.bhmod"), b"zip?").unwrap();
    std::fs::write(own.join("mods.toml"), "[off]\nenabled = false\n[mine]\ngrant = [\"channels\"]\n").unwrap();
    // Developer folders: a good one, one with the server's id, a broken one and a missing one.
    let dev = |name: &str, id: &str, entry: &str| -> PathBuf {
        let p = d.path().join(name);
        std::fs::create_dir_all(p.join("client/lib")).unwrap();
        std::fs::write(
            p.join("mod.toml"),
            format!("schema = 1\nid = \"{id}\"\nversion = \"0.1.0-dev\"\nname = {{ en = \"D\" }}\nauthors = [\"a\"]\nlicense = \"MIT\"\napi = \"^0.2\"\n[sides]\nclient = true\n[entry.client]\nruntime = \"lua\"\npath = \"{entry}\"\n"),
        )
        .unwrap();
        std::fs::write(p.join("client/main.luau"), "return {}\n").unwrap();
        std::fs::write(p.join("client/lib/x.luau"), "return 1\n").unwrap();
        p
    };
    let mut plan = server_plan(d.path());
    plan.own = Some(own.clone());
    plan.dev = vec![
        dev("devmod", "devmod", "client/main.luau"),
        dev("dup", "echo", "client/main.luau"),
        dev("bad", "badd", "client/nope.luau"),
        d.path().join("missing"),
    ];
    let session = d.path().join("session");
    std::fs::create_dir_all(session.join("stale")).unwrap();
    let (placed, problems) = build(&session, &plan).unwrap();
    let ids: Vec<(&str, &str)> = placed.iter().map(|p| (p.id.as_str(), p.origin)).collect();
    assert_eq!(ids, [("echo", "server"), ("devmod", "dev"), ("mine", "mine")]);
    assert_eq!(problems.len(), 5, "{problems:?}");
    assert!(!session.join("stale").exists());
    assert!(session.join("d00-devmod/client/lib/x.luau").is_file());
    let toml = std::fs::read_to_string(session.join("mods.toml")).unwrap();
    assert!(toml.contains("[mine]") && toml.contains("[echo]"), "{toml}");
    // A broken mods.toml is reported and no grants are taken from it.
    std::fs::write(own.join("mods.toml"), "[[[").unwrap();
    let (_, problems) = build(&session, &Plan { own: Some(own), ..Default::default() }).unwrap();
    assert!(problems.iter().any(|p| p.contains("mods.toml")), "{problems:?}");
    // The session folder cannot be made.
    let file = d.path().join("file");
    std::fs::write(&file, b"").unwrap();
    assert!(build(&file.join("x"), &Plan::default()).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// The timing wrapper forwards every guest call.

struct FakeRt;
struct FakeInst;

impl Runtime for FakeRt {
    fn kind(&self) -> bhmod_manifest::manifest::Runtime {
        bhmod_manifest::manifest::Runtime::Wasm
    }
    fn instantiate(&self, id: &str, _f: &ModFiles, _l: InstanceLimits) -> Result<Box<dyn Instance>, String> {
        if id == "nope" {
            return Err("no".into());
        }
        Ok(Box::new(FakeInst))
    }
}

impl Instance for FakeInst {
    fn on_enable(&mut self, _h: &mut dyn HostApi) -> Result<(), CallError> {
        Ok(())
    }
    fn on_disable(&mut self, _h: &mut dyn HostApi) -> Result<(), CallError> {
        Ok(())
    }
    fn handle_event(&mut self, _h: &mut dyn HostApi, _s: u32, _e: &Event) -> Result<Outcome, CallError> {
        Ok(Outcome::default())
    }
    fn run_command(
        &mut self,
        _h: &mut dyn HostApi,
        _id: u32,
        _s: &CommandSender,
        _l: &str,
        _a: &[String],
    ) -> Result<bhmod_core::types::CommandOutcome, CallError> {
        Err(CallError::Failed("x".into()))
    }
    fn run_task(&mut self, _h: &mut dyn HostApi, _id: u64) -> Result<(), CallError> {
        Ok(())
    }
    fn call_service(&mut self, _h: &mut dyn HostApi, _n: &str, _m: &str, a: &[u8]) -> Result<Vec<u8>, CallError> {
        Ok(a.to_vec())
    }
}

struct NoHost;

#[allow(unused_variables)]
impl HostApi for NoHost {
    fn deadline(&self) -> Instant {
        Instant::now()
    }
    fn api_version(&self) -> String {
        unimplemented!()
    }
    fn mod_id(&self) -> String {
        unimplemented!()
    }
    fn mod_version(&self) -> String {
        unimplemented!()
    }
    fn current_side(&self) -> bhmod_core::types::Side {
        unimplemented!()
    }
    fn host_name(&self) -> String {
        unimplemented!()
    }
    fn log(&mut self, level: bhmod_core::types::LogLevel, message: &str) {}
    fn player_alias(&mut self, id: &str) -> Result<String, ApiError> {
        unimplemented!()
    }
    fn player_is_online(&mut self, id: &str) -> bool {
        unimplemented!()
    }
    fn player_role(&mut self, id: &str) -> Result<bhmod_core::types::Role, ApiError> {
        unimplemented!()
    }
    fn player_send_chat(&mut self, id: &str, text: &str) -> Result<(), ApiError> {
        unimplemented!()
    }
    fn player_kick(&mut self, id: &str, reason: &str) -> Result<(), ApiError> {
        unimplemented!()
    }
    fn players_get(&mut self, id: &str) -> Option<String> {
        unimplemented!()
    }
    fn players_find_by_alias(&mut self, alias: &str) -> Option<String> {
        unimplemented!()
    }
    fn players_online(&mut self) -> Vec<String> {
        unimplemented!()
    }
    fn players_broadcast(&mut self, text: &str) -> Result<(), ApiError> {
        unimplemented!()
    }
    fn subscribe(&mut self, kind: EventKind, priority: Priority, ignore_cancelled: bool) -> Result<u32, ApiError> {
        unimplemented!()
    }
    fn unsubscribe(&mut self, sub: u32) -> bool {
        unimplemented!()
    }
    fn register_command(&mut self, spec: bhmod_core::types::CommandSpec) -> Result<u32, ApiError> {
        unimplemented!()
    }
    fn unregister_command(&mut self, id: u32) -> bool {
        unimplemented!()
    }
    fn run_later(&mut self, delay: u32) -> Result<u64, ApiError> {
        unimplemented!()
    }
    fn run_every(&mut self, delay: u32, period: u32) -> Result<u64, ApiError> {
        unimplemented!()
    }
    fn run_next_poll(&mut self) -> Result<u64, ApiError> {
        unimplemented!()
    }
    fn cancel_task(&mut self, id: u64) -> bool {
        unimplemented!()
    }
    fn storage_get(&mut self, key: &str) -> Option<Vec<u8>> {
        unimplemented!()
    }
    fn storage_set(&mut self, key: &str, value: &[u8]) -> Result<(), ApiError> {
        unimplemented!()
    }
    fn storage_delete(&mut self, key: &str) -> bool {
        unimplemented!()
    }
    fn storage_keys(&mut self, prefix: &str) -> Vec<String> {
        unimplemented!()
    }
}

#[test]
fn timed_wrapper_forwards_and_counts() {
    let (rt, timing) = timed_for_tests(Box::new(FakeRt));
    assert_eq!(rt.kind(), bhmod_manifest::manifest::Runtime::Wasm);
    let files = ModFiles::default();
    let lim = InstanceLimits { memory_bytes: 1 };
    assert!(rt.instantiate("nope", &files, lim).is_err());
    let mut i = rt.instantiate("m", &files, lim).unwrap();
    let h = &mut NoHost;
    h.log(bhmod_core::types::LogLevel::Info, "x");
    i.on_enable(h).unwrap();
    i.handle_event(h, 1, &Event::Tick { tick: 1, dt: 0.0 }).unwrap();
    assert!(i.run_command(h, 1, &CommandSender::Console, "x", &[]).is_err());
    i.run_task(h, 1).unwrap();
    assert_eq!(i.call_service(h, "s", "m", b"ab").unwrap(), b"ab");
    i.on_channel_message(h, 1, &ChannelSource::Server, b"").unwrap();
    i.on_disable(h).unwrap();
    // Seven calls; a call in a new second moves the window.
    let u = usage_for_tests(&timing, "m");
    assert_eq!(u.calls, 7);
    add_for_tests(&timing, "m", Instant::now() + Duration::from_secs(2), Duration::from_millis(3));
    let u = usage_for_tests(&timing, "m");
    assert!(u.ms_last_second < 3.0 && u.peak_ms >= 3.0);
}

#[test]
fn the_jvm_is_opt_in() {
    let d = tempfile::tempdir().unwrap();
    let (rts, note) = session_runtimes(d.path(), false, None);
    assert_eq!((rts.len(), note), (2, None));
    let (rts, note) = session_runtimes(d.path(), true, None);
    let note = note.unwrap();
    if JVM_BUILT {
        // Built with the JVM and a JRE on this machine: a third runtime, and the SDK jar is written for its class path.
        assert_eq!(rts.len(), 3, "{note}");
        assert!(note.starts_with("JVM mods are on:"), "{note}");
        assert!(std::fs::read_dir(d.path().join("jvm"))
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("bhmod-sdk-")));
    } else {
        assert_eq!(rts.len(), 2);
        assert!(note.contains("no JVM support"), "{note}");
    }
}

const WATCHER: &str = r#"
local function at(p) return p.x .. "," .. p.y end
local function who(p) return p and p:alias() or "-" end
bh.events.on("world-save", function(ev) bh.log.info("save " .. ev.phase) end)
bh.events.on("block-place", function(ev)
    bh.log.info("place " .. who(ev.player) .. " " .. at(ev.pos) .. " " .. ev.item .. " " .. ev.data_a .. " " .. ev.data_b)
    ev.cancelled = true
end)
bh.events.on("block-break", function(ev)
    bh.log.info("break " .. who(ev.player) .. " " .. at(ev.pos) .. " " .. ev.layer .. " " .. ev.tile)
end)
bh.events.on("block-changed", function(ev) bh.log.info("changed " .. at(ev.pos) .. " " .. ev.tile .. " " .. who(ev.player)) end)
bh.events.on("object-place", function(ev) bh.log.info("object " .. who(ev.player) .. " " .. ev.object .. " " .. ev.item) end)
bh.events.on("damage", function(ev) bh.log.info("damage " .. who(ev.player) .. " " .. ev.blockhead .. " " .. ev.amount) end)
bh.events.on("death", function(ev) bh.log.info("death " .. who(ev.player) .. " " .. ev.blockhead) end)
bh.events.on("player-move", function(ev) bh.log.info("move " .. ev.player:alias() .. " " .. at(ev.from) .. " " .. at(ev.to)) end)
bh.commands.register({ name = "where" }, function(sender)
    local p = sender.player:pos()
    return { handled = true, reply = p and at(p) or "nowhere" }
end)
return {}
"#;

#[test]
fn world_events_positions_and_a_single_player() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("watch.bhmod");
    std::fs::write(&p, pkg("watch", "1.0.0", WATCHER, "", 0)).unwrap();
    let plan = Plan { own: None, dev: vec![], server: vec![Accepted { id: "watch".into(), path: p, grant: vec![] }] };
    let (mut h, _, _) = setup(d.path(), false, plan);
    // Nothing runs before the world is ready.
    h.world_saved(true);
    assert!(!h.watching());
    h.enable("W", "w");
    assert!(h.watching());
    h.take_new_logs();
    // (A slow call adds a budget warning of the host; only the mod's own lines count.)
    let logs = |h: &ClientHost| {
        h.take_new_logs()
            .into_iter()
            .filter(|l| l.mod_id.as_deref() == Some("watch") && !l.text.contains("budget"))
            .map(|l| l.text)
            .collect::<Vec<_>>()
    };
    h.world_saved(true);
    h.world_saved(false);
    assert_eq!(logs(&h), ["save pre", "save post"]);
    // The player is in their own game even offline; others only once the server lists them.
    h.players(vec!["BOB".into()]);
    h.block_placed("ME", (3, 4), 11, (1, 2));
    h.block_placed("GHOST", (5, 6), 12, (0, 0));
    assert_eq!(logs(&h), ["place ME 3,4 11 1 2", "changed 3,4 11 ME", "changed 5,6 12 -"]);
    h.block_broken("BOB", (3, 4), false);
    h.block_broken("BOB", (3, 4), false);
    h.block_broken("BOB", (3, 4), true);
    h.block_broken("GHOST", (9, 9), false);
    assert_eq!(
        logs(&h),
        [
            "break BOB 3,4 block 11",
            "changed 3,4 0 BOB",
            "break BOB 3,4 block 0",
            "changed 3,4 0 BOB",
            "break BOB 3,4 back-wall 0",
            "changed 9,9 0 -"
        ]
    );
    h.object_placed("BOB", (1, 1), true, 7);
    h.object_placed("ME", (2, 2), false, 40);
    h.object_placed("GHOST", (2, 2), false, 40);
    h.damaged(Some("BOB"), 77, 0.25);
    h.damaged(Some("GHOST"), 78, 0.5);
    h.damaged(None, 79, 1.0);
    h.died(Some("ME"), 5);
    h.died(None, 6);
    assert_eq!(
        logs(&h),
        [
            "object BOB workbench 7",
            "object ME interaction-object 40",
            "damage BOB 77 0.25",
            "damage - 78 0.5",
            "damage - 79 1",
            "death ME 5",
            "death - 6"
        ]
    );
    // `player:pos()` follows what the game says; `player-move` only fires for a change.
    assert!(h.typed("/where"));
    assert_eq!(h.take_effects(), vec![Effect::Chat("nowhere".into())]);
    h.moved("ME", (10, 20));
    h.moved("ME", (10, 20));
    h.moved("ME", (11, 20));
    h.moved("GHOST", (0, 0));
    h.moved("GHOST", (1, 0));
    assert_eq!(logs(&h), ["move ME 10,20 11,20"]);
    assert!(h.typed("/where"));
    assert_eq!(h.take_effects(), vec![Effect::Chat("11,20".into())]);
    // The tile table starts over when it is full.
    for i in 0..66 {
        h.block_placed("ME", (i, 0), 1, (0, 0));
    }
    h.take_new_logs();
    h.block_broken("ME", (65, 0), false);
    assert!(logs(&h)[0].ends_with("block 1"));
    h.block_broken("ME", (0, 0), false);
    assert!(logs(&h)[0].ends_with("block 0"));
}
