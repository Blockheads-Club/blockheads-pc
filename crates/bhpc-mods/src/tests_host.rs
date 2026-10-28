use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bhmod_core::types::{ApiError, ChannelTarget};
use bhmod_core::GameBridge;

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
        voice: None,
        binds: None,
        client_settings: None,
        hud: None,
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

const SVC: &str = r#"
local mod = {}
function mod.on_enable()
    bh.services.provide("hud", "1.0.0", {
        title = function(n)
            local c = bh.services.caller()
            return "title " .. n .. " from " .. c.mod .. " " .. tostring(c.player)
        end,
    })
    local function ask(item)
        bh.services.call_remote("server", "prices", "^1", "price", { item }, function(ok, v)
            bh.log.info("answer " .. item .. " " .. tostring(ok) .. " " .. (ok and tostring(v) or v.kind))
        end)
    end
    bh.events.on("tick", function(ev)
        if ev.tick == 1 then ask("a") elseif ev.tick == 2 then ask("b") elseif ev.tick == 3 then ask("c") end
    end)
end
return mod
"#;

fn svc_host(dir: &Path, online: bool) -> ClientHost {
    let extra = "[permissions]\nchannels = true\n[provides]\nservices = [{ name = \"hud\", version = \"1.0.0\", wit = \"wit/svc.wit\", remote = true }]";
    let p = dir.join("svc.bhmod");
    std::fs::write(&p, pkg("svc", "1.0.0", SVC, extra, 0)).unwrap();
    let plan = Plan {
        own: None,
        dev: vec![],
        server: vec![Accepted { id: "svc".into(), path: p, grant: vec!["channels".into()] }],
    };
    let (mut h, _, problems) = setup(dir, online, plan);
    assert!(problems.is_empty(), "{problems:?}");
    h.enable("W", "w");
    h
}

#[test]
fn service_calls_both_ways() {
    use bhmod_proto::{service_error, ServiceCall, ServiceResult};
    let d = tempfile::tempdir().unwrap();
    let mut h = svc_host(d.path(), true);
    assert_eq!(h.status()[0].state, "running", "{:?}", h.log());
    // A mod's call leaves as an effect; its answers, failures and the connection loss come back as events.
    let t = Instant::now();
    h.tick(t);
    h.tick(t);
    h.tick(t);
    let effects = h.take_effects();
    let calls: Vec<_> = effects
        .iter()
        .map(|e| match e {
            Effect::Service(c) => (c.id, c.service.clone(), c.method.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        calls,
        vec![
            (1, "prices".into(), "price".into()),
            (2, "prices".into(), "price".into()),
            (3, "prices".into(), "price".into())
        ]
    );
    assert!(h.service_result_in(ServiceResult { id: 1, outcome: Ok(vec![0x03]) }));
    h.service_unavailable(2);
    h.server_gone();
    let logs: Vec<String> = h.take_new_logs().into_iter().map(|l| l.text).collect();
    for want in ["answer a true 3", "answer b false unavailable", "answer c false disconnected"] {
        assert!(logs.iter().any(|l| l == want), "{want}: {logs:?}");
    }
    assert!(!h.service_result_in(ServiceResult { id: 1, outcome: Ok(vec![0x03]) }), "already answered");
    // The server calls a client mod's service (remote = true): it sees the server as the caller.
    let call = ServiceCall {
        id: 9,
        mod_id: "srv".into(),
        service: "hud".into(),
        version_req: "^1".into(),
        method: "title".into(),
        args: vec![0x81, 0x07],
    };
    let r = h.service_call_in(call.clone());
    let want = bhmod_core::value::Value::text("title 7 from srv nil").encode();
    assert_eq!(r, ServiceResult { id: 9, outcome: Ok(want) });
    let r = h.service_call_in(ServiceCall { id: 10, service: "nothing".into(), ..call });
    assert_eq!(r.id, 10);
    assert_eq!(r.outcome.unwrap_err().code, service_error::NOT_FOUND);
    h.stop();
}

#[test]
fn offline_service_calls_are_unavailable() {
    let d = tempfile::tempdir().unwrap();
    let mut h = svc_host(d.path(), false);
    h.tick(Instant::now());
    assert!(h.take_effects().is_empty());
    assert!(h.take_new_logs().iter().any(|l| l.text == "answer a false unavailable"));
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
// Hot reload and the time accounting.

#[test]
fn hot_reload_restarts_only_what_changed_and_timings_reset() {
    let d = tempfile::tempdir().unwrap();
    let plan = server_plan(d.path());
    let (mut h, _, _) = setup(d.path(), true, plan.clone());
    h.enable("World", "w1");
    assert!(h.typed("/hello"));
    let st = h.status();
    let t = &st[0].timings;
    assert_eq!(t.id, "echo");
    assert!(t.calls() >= 2, "on-enable and the command: {t:?}");
    assert_eq!(st[0].usage.calls, t.calls());
    assert!(st[0].usage.peak_ms >= 0.0 && st[0].usage.ms_last_second >= 0.0);
    // Nothing changed on disk: nothing happens.
    build(&d.path().join("session"), &plan).unwrap();
    assert!(h.reload(None).is_empty());
    // A newer package: the session folder is built again, the mod restarts, the player stays in the world.
    let newer = pkg(
        "echo",
        "1.1.0",
        &MAIN.replace("\"hi \"", "\"yo \""),
        "[permissions]\nchannels = true\nplayers = [\"chat\"]",
        0,
    );
    std::fs::write(&plan.server[0].path, newer).unwrap();
    build(&d.path().join("session"), &plan).unwrap();
    h.take_new_logs();
    let r = h.reload(None);
    assert_eq!(r, ["echo: reloaded (1.0.0 -> 1.1.0)"]);
    assert!(h.take_new_logs().iter().any(|l| l.text == "enabled"));
    assert!(h.typed("/hello"));
    assert!(h.take_effects().contains(&Effect::Chat("yo 0 0".into())));
    assert_eq!(h.status()[0].version, "1.1.0");
    // The accounting restarted with the mod, and can be cleared.
    assert!(h.reset_timings(Some("echo")));
    assert_eq!(h.status()[0].timings.calls(), 0);
    assert!(!h.reset_timings(Some("ghost")));
    assert!(h.reset_timings(None));
    h.stop();
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
