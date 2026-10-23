# Client mods on Blockheads PC (mod system milestone M6)

The PC client runs Luau and WASM client mods (mod-sdk `bhmod-core` + `bhmod-wasm`, pinned in the workspace
`Cargo.toml`) and speaks the 0xBC mod protocol (mod-sdk `docs/protocol-0xbc.md`) with modded Blockheads Servers.

## Pieces

| Where | What |
| --- | --- |
| `crates/bhpc-mods/src/join.rs` | `JoinFlow`: mod-sdk's sans-IO `bhmod_client::Session` (the client side of 0xBC, shared with the Android loader: HELLO after 0x23, 1500 ms vanilla fallback, `LIST_REQUEST`/`MOD_LIST`, downloads in list order with an acknowledgement every 4 chunks, resume from `.part`, BUSY retry, 20 s stall, SHA-256 and the package rules, `READY`, channel ids) wrapped for the PC glue: the prompt's entries with the index check (native code needs an index-listed file with the same SHA-256 *and* the player's confirmation, never with "block native"), the per-mod switch of optional mods (`toggle`; required mods are fixed; `READY.loaded` carries the chosen subset and the server stops sending a declined mod's channel traffic), the time left, `bhcModLoader` injected into the held 0x1f (`inject_mod_loader`) and the launcher's cache (`mods-cache/blobs/<sha256>.bhmod`; `record_join` writes `mods-cache/servers/<key>.toml`). |
| `crates/bhpc-mods/src/session.rs` | The session folder `mods-cache/session/` that `ModHost` loads: server mods (accepting grants what they ask), developer folders (`--mods-dev`), the player's enabled mods (`mods/`, `mods.toml` grants). A server's id wins. |
| `crates/bhpc-mods/src/host.rs` | `ClientHost`: `ModHost` with the PC `GameBridge`, effects (chat lines, channel packets) carried out by the glue, a 500-line log, per-mod time (`ms` per second, longest call). |
| `crates/bhpc-mods/src/ui.rs`, `ui/join.rs`, `view.rs` | The in-game UI. The join dialog follows the design canvas (PC-Mods-Join: server tile, status tag, address, mod rows with sides, permissions, size and a switch for optional mods, the danger zone for native code with its confirmation box, the download bar with the time left, notices for a stopped download, a failed file check with "Copy details", a newer mod loader and a refused join; key hints on the buttons; Up/Down/Space step through the switches). Not in it yet: "Try again" (no retry path), "Update after this game", the players online in the address line, the hash of the file as received. The F9 panel, toasts and the pixel kit (IG-Spec, font Tiny5, OFL, `crates/bhpc-mods/fonts`) are unchanged. Drawn into a premultiplied RGBA picture; the dialog's text is the bundled Liberation Sans. |
| `crates/bhpc-run/src/modglue.rs`, `modevents.rs` (play build) | ObjC hooks (`objchook`) and the main-thread state; `modevents.rs` holds the world-event hooks. |
| `crates/bhpc-run/src/overlay.rs` `Composer` | Blends the mod UI over the Java views' overlay: one path for the launcher's embedded view and the SDL window; screenshots include it. |
| `crates/bhpc-launcher/src/mods/live.rs` | `@mods` report parsing, live status on the Mods screens, runner flags from the settings. |

## Hooks (1.7.x)

- `-[BHClient match:didReceiveData:fromPlayer:]`: 0x23 starts a join and sends `HELLO`; 0xBC goes to the flow and
  never reaches the game.
- `-[BHNetClientMatch sendData:toPlayers:withDataMode:error:]`: the game's 0x1f (`0x1f` + XML plist) is *held*
  (answered YES, not sent) while the list is open and sent from the frame hook with `bhcModLoader` once `READY` is out.
  Nothing waits on the game thread.
- `-[World update:accurateDT:pinchScale:dragInProgress:]` starts the session at the first update of a world
  (not while a join negotiates: that update is the world being left) and fires `tick`; `-[World dealloc]` stops it.
- `-[BHClient chatMessageRecieved:displayNotification:]` (argument: `{message, alias, ...}` dictionary): `chat`
  event; cancel hides it, a changed text replaces `message`. Mod chat lines are shown through it as alias `MOD`.
- `-[BHClient sendChatMessage:sendToClients:]`: typed `/commands` of mods run on the client and are not sent.
- `-[BHNetNode updatePlayerList:]` (after): `player-join` / `player-quit` from `livePlayerInfos`.
- `-[MainMenuUI render:projectionMatrix:]` (explicit signature: the matrix is a union): `--join HOST:PORT` waits
  until the menu has shown for 3 s, then `selectJoinOption`, `gameLaunchedWithUrlToJoinHost:port:`,
  `joinOnlineWorldButtonTapped`.

Client API events now: `chat`, `command` + mod commands, `tick`, `player-join`, `player-quit`, `world-loaded`,
`world-save`, `block-place`, `block-break`, `block-changed`, `object-place`, `damage`, `death`, `player-move`,
`player:pos()`, channels to and from the server, `player.send-chat` / `players.broadcast` (shown locally). The world
events come from `crates/bhpc-run/src/modevents.rs` (hooks listed there) and are notifications: the server decides
what happens in a world, so a client mod's `cancelled` or changed `amount` is dropped. A player is named by the
blockhead's or the placing client's name; the local player's own actions carry none, so they read as the local player.
Not on a client: `player-pre-login`. Still open: JVM and native client mods (M9/M10), content (M8).

## Runner flags and launcher protocol

`bhpc-run` (play): `--no-mods`, `--mods-dev DIR` (repeatable), `--mods-block-native`, `--mods-auto-accept` (tests),
`--join HOST:PORT`, `--control` (a game in a window of its own reads the launcher's lines on stdin: `mods-reload`,
`mods-chat TEXT` to say a line in the chat as if typed; the launcher adds it to every game that is not embedded, which
already reads all its commands from stdin). Mod files live below the data folder (`mods/`, `mods-cache/`, `mods-data/<server key>/` for mod
storage). Stdout: `@mods [{id, version, state, origin, ms, peak_ms, calls, reason}]` when it changes; stderr: `[mods]`
log lines. Stdin (embedded view, or `--control`): `mods-reload`. In the game: F9 opens the Mods panel, F5 reloads folder mods.

## Tests

- `crates/bhpc-mods`: unit tests with fixture packages and real Luau (100 % lines/functions);
  `BHPC_MODS_UI_DUMP=DIR cargo test -p bhpc-mods tests_ui` writes every UI state as `.pam` pictures.
- `scripts/e2e-mods.sh`: the real game headless against a local blockheads-server (`BHPC_SERVER_BIN`, or
  `BHPC_SERVER_SRC` to build one) with `fixtures/mods/echo` (client + server, channel echo, server chat): prompt,
  Enter, download, join, the client mod runs, channel round trip, the server's chat reaches the client mod, F9 panel,
  a second join through the game's Join form from the cache, and a vanilla server (no `SERVER_HELLO`, join after
  1.5 s). The server ends by itself when its last player leaves; the script restarts it.

## JVM client mods (Java, Kotlin; milestone M9)

Off by default, twice: build the runner with `--features jvm` (`cargo build --release -p bhpc-run --features play,jvm`;
needs a JDK, the Java SDK is compiled in) and start it with `--mods-jvm` (the launcher passes it when the preference
`mods_jvm` is true in `prefs.json`; `mods_java_home` names a JRE, otherwise `BHMOD_JAVA_HOME`, `JAVA_HOME` and `java` on
the `PATH` are tried). The runtime is mod-sdk's `bhmod-core` JVM runtime ([`docs/jvm.md`](https://github.com/Blockheads-Club/mod-sdk/blob/dev/docs/jvm.md)):
one JVM per process on the user's JRE, one class loader per mod, `client/mod.jar` with `Bhmod-Main` in its manifest.
A JVM mod is `native` (unsandboxed): the join prompt shows the red native zone, it is refused with "never allow native
mods", and a server can only push one that the index lists with the same SHA-256 and the player confirmed. With the JVM
on, `HELLO.runtimes` also lists `jvm`. There is no Settings toggle yet: edit `prefs.json`.

`cargo test -p bhpc-mods --features jvm` runs a Java client mod through `ClientHost` (needs a JDK and a JRE); CI has a
job for it.
