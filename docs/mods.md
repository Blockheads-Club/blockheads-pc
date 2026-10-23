# Client mods on Blockheads PC (mod system milestone M6)

The PC client runs Luau and WASM client mods (mod-sdk `bhmod-core` + `bhmod-wasm`, pinned in the workspace
`Cargo.toml`) and speaks the 0xBC mod protocol (mod-sdk `docs/protocol-0xbc.md`) with modded Blockheads Servers.

## Pieces

| Where | What |
| --- | --- |
| `crates/bhpc-mods/src/join.rs` | `JoinFlow`: the client side of 0xBC as a state machine without game I/O. HELLO after 0x23, 1500 ms vanilla fallback, `LIST_REQUEST`/`MOD_LIST`, prompt, downloads in list order (ack every 4 chunks, resume from `.part`, BUSY retry, 20 s stall), SHA-256 + package rules (`bhpc_launcher::mods::install::inspect`) + native refusal (index-listed with the same SHA-256 *and* confirmed, never with "block native"), `READY`, `bhcModLoader` injected into the held 0x1f (`inject_mod_loader`), channel ids from `MOD_LIST` and `CHANNEL_REGISTER`, `REJECT_TEXT`. Cache: the launcher's `mods-cache/blobs/<sha256>.bhmod`; `record_join` writes `mods-cache/servers/<key>.toml`. |
| `crates/bhpc-mods/src/session.rs` | The session folder `mods-cache/session/` that `ModHost` loads: server mods (accepting grants what they ask), developer folders (`--mods-dev`), the player's enabled mods (`mods/`, `mods.toml` grants). A server's id wins. |
| `crates/bhpc-mods/src/host.rs` | `ClientHost`: `ModHost` with the PC `GameBridge`, effects (chat lines, channel packets) carried out by the glue, a 500-line log, per-mod time (`ms` per second, longest call). |
| `crates/bhpc-mods/src/ui.rs`, `view.rs` | The in-game UI in the IG pixel kit (IG-Spec/IG-Kit/IG-Join/IG-Mods/IG-HUD; font Tiny5, OFL, `crates/bhpc-mods/fonts`): join prompt (permissions, "Not reviewed", red native zone with a checkbox), downloading, download/file-check failure, the server's rejection text, the F9 panel (state, origin, time, log), toasts ("… was turned off. It slowed the game."). Drawn into a premultiplied RGBA picture. |
| `crates/bhpc-run/src/modglue.rs` (play build) | ObjC hooks (`objchook`) and the main-thread state. |
| `crates/bhpc-run/src/cosmetics.rs` (play build) | M8: the cosmetics hooks on `Blockhead` (see below). What a blockhead shows is decided by mod-sdk's `bhmod-content`, shared with the Android loader. |
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

## Cosmetics (M8)

A server mod that declares `[content] cosmetics` (mod-sdk `docs/content.md`) re-skins what a player wears (auto-wear: an empty clothing slot gets a plain vanilla item to draw on). The join
flow keeps `MOD_LIST.registry` and hands `COSMETICS` (0xBC op 0x12, after the join) to
`bhpc-run/src/cosmetics.rs`; the session start reads the cosmetics of the session's packages
(`bhmod_content::Content`). Hooks (main thread, inactive while no cosmetic is declared):

- `-[Blockhead updateClothingCubes]` (this client's own blockheads): original, then the texture ivars
  (`hatTexture`, `shirtBodyTexture`, `shirtArmTexture`, `pantsTexture`, `shoesTexture`) are replaced with
  `CPTexture2D`s made from the mod's PNG (`initWithData:pixelFormat:1:pixelsWide:pixelsHigh:contentSize:mipmap:`,
  the game's own loader path; PNGs are padded to powers of two and premultiplied RGBA made by `bhmod-content`).
- Auto-wear: before the original `updateClothingCubes` runs, the empty clothing slots a cosmetic needs get a plain
  vanilla `InventoryItem` (put into `inventoryItems[0].subItems[slot]` for the call, and its type into the
  `*ItemType` ivar: the armv7 build builds its cubes from those); the item type then travels in the blockhead's
  creation data, so other players draw the shape too.
- `-[Blockhead updateSkin]` (hair texture `headHairTexture`: a mask, see mod-sdk `docs/content.md`), `-[Blockhead setupFromNetCreationData:]` (other
  players' blockheads: clothing and hair), `-[Blockhead update:accurateDT:isSimulation:]` (refresh when the cosmetics changed).
- Dev aids: `BHPC_DEV_COSMETICS=hat=<key>,hair=<key>` puts cosmetics of a `--mods-dev` folder on every blockhead in a
  single-player world (ids from an in-memory registry); `BHPC_COSMETICS_TRACE=1` logs what the hooks do.

Client API events now: `chat`, `command` + mod commands, `tick`, `player-join`, `player-quit`, `world-loaded`,
channels to and from the server, `player.send-chat` / `players.broadcast` (shown locally). Deferred (no client hook
yet): `world-save`, `player-pre-login` (server only), block/object/damage/death/move events, `player.pos`, services
between client and server (only local), JVM and native client mods (M9/M10), content (M8).

## Runner flags and launcher protocol

`bhpc-run` (play): `--no-mods`, `--mods-dev DIR` (repeatable), `--mods-block-native`, `--mods-auto-accept` (tests),
`--join HOST:PORT`. Mod files live below the data folder (`mods/`, `mods-cache/`, `mods-data/<server key>/` for mod
storage). Stdout: `@mods [{id, version, state, origin, ms, peak_ms, calls, reason}]` when it changes; stderr: `[mods]`
log lines. Stdin (embedded view): `mods-reload`. In the game: F9 opens the Mods panel, F5 reloads folder mods.

## Tests

- `crates/bhpc-mods`: unit tests with fixture packages and real Luau (100 % lines/functions);
  `BHPC_MODS_UI_DUMP=DIR cargo test -p bhpc-mods tests_ui` writes every UI state as `.pam` pictures.
- `scripts/e2e-mods.sh`: the real game headless against a local blockheads-server (`BHPC_SERVER_BIN`, or
  `BHPC_SERVER_SRC` to build one) with `fixtures/mods/echo` (client + server, channel echo, server chat): prompt,
  Enter, download, join, the client mod runs, channel round trip, the server's chat reaches the client mod, F9 panel,
  a second join through the game's Join form from the cache, and a vanilla server (no `SERVER_HELLO`, join after
  1.5 s). The server ends by itself when its last player leaves; the script restarts it.
