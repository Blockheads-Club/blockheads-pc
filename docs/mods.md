# Client mods on Blockheads PC (mod system milestone M6)

The PC client runs Luau and WASM client mods (mod-sdk `bhmod-core` + `bhmod-wasm`, pinned in the workspace
`Cargo.toml`) and speaks the 0xBC mod protocol (mod-sdk `docs/protocol-0xbc.md`) with modded Blockheads Servers.

## Pieces

| Where | What |
| --- | --- |
| `crates/bhpc-mods/src/join.rs` | `JoinFlow`: mod-sdk's sans-IO `bhmod_client::Session` (the client side of 0xBC, shared with the Android loader: HELLO after 0x23, 1500 ms vanilla fallback, `LIST_REQUEST`/`MOD_LIST`, downloads in list order with an acknowledgement every 4 chunks, resume from `.part`, BUSY retry, 20 s stall, SHA-256 and the package rules, `READY`, channel ids) wrapped for the PC glue: the prompt's entries with the index check (native code needs an index-listed file with the same SHA-256 *and* the player's confirmation, never with "block native"), the per-mod switch of optional mods (`toggle`; required mods are fixed; `READY.loaded` carries the chosen subset and the server stops sending a declined mod's channel traffic), the time left, `bhcModLoader` injected into the held 0x1f (`inject_mod_loader`) and the launcher's cache (`mods-cache/blobs/<sha256>.bhmod`; `record_join` writes `mods-cache/servers/<key>.toml`). |
| `crates/bhpc-mods/src/session.rs` | The session folder `mods-cache/session/` that `ModHost` loads: server mods (accepting grants what they ask), developer folders (`--mods-dev`), the player's enabled mods (`mods/`, `mods.toml` grants). A server's id wins. |
| `crates/bhpc-mods/src/host.rs` | `ClientHost`: `ModHost` with the PC `GameBridge`, effects (chat lines, channel packets) carried out by the glue, a 500-line log, per-mod time (`ms` per second, longest call). |
| `crates/bhpc-mods/src/ui.rs`, `ui/join.rs`, `view.rs` | The in-game UI. The join dialog follows the design canvas (PC-Mods-Join: server tile, status tag, address, mod rows with sides, permissions, size and a switch for optional mods, the danger zone for native code with its confirmation box, the download bar with the time left, notices for a stopped download, a failed file check with "Copy details", a newer mod loader and a refused join; key hints on the buttons; Up/Down/Space step through the switches). Also: "Try again" after a stopped download (`JoinFlow::retry`: the connection stays open on that failure, a fresh session says `HELLO` again, the reference server answers it until the join is approved, the player's earlier choice is replayed when the list arrives and the partial files resume; if nobody answers within 1.5 s the join fails again instead of going out as vanilla; leaving the notice closes the connection); "Update after this game" on a newer-loader notice (see below); the players online in the address line, drawn when the server's `SERVER_HELLO` carries the optional `players` / `maxPlayers` keys (mod-sdk protocol, older servers omit them: then nothing is drawn) or `JoinFlow::set_players` was called; and the hash of the file as received ("got sha256 ...", taken from the part file plus the last chunk before the session checks and deletes the file; also in "Copy details"). The F9 panel, toasts and the pixel kit (IG-Spec, font Tiny5, OFL, `crates/bhpc-mods/fonts`) are unchanged. Drawn into a premultiplied RGBA picture; body text is Atkinson Hyperlegible Next (OFL, `third_party/fonts`, with its licence and `NOTICE.md`), with Liberation Sans for bold (no bold Atkinson file is bundled) and for glyphs Atkinson lacks (Cyrillic); the design's Bricolage Grotesque is not bundled. |
| `crates/bhpc-run/src/modglue.rs`, `modevents.rs` (play build) | ObjC hooks (`objchook`) and the main-thread state; `modevents.rs` holds the world-event hooks. |
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
`world-save`, `block-place`, `block-break`, `block-changed`, `object-place`, `damage`, `death`, `player-move`,
`player:pos()`, channels to and from the server, `player.send-chat` / `players.broadcast` (shown locally). The world
events come from `crates/bhpc-run/src/modevents.rs` (hooks listed there) and are notifications: the server decides
what happens in a world, so a client mod's `cancelled` or changed `amount` is dropped. A player is named by the
blockhead's or the placing client's name; the local player's own actions carry none, so they read as the local player.
Service calls (API 0.3.1, mod-sdk `docs/luau-api.md#remote-services-api-031`): a client mod calls a service a server
mod provides with `bh.services.call_remote("server", ...)`, asynchronously; the call leaves as `Effect::Service` (a `SERVICE_CALL`
packet), the answer (`SERVICE_RESULT`, `Action::ServiceResultIn`) reaches the mod's callback, a call that cannot be sent
(single-player, a server that has not announced mods) is answered `unavailable`. The server can call a client mod's
service too (`remote = true` in its `[provides]`; `Action::ServiceCallIn` runs it under the mod's budget and the glue
sends the `SERVICE_RESULT`).
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
on, `HELLO.runtimes` also lists `jvm`. Settings > Mods > "Java and Kotlin mods" has the switch (default off, with a red warning that JVM mods are native and cannot be sandboxed) and an optional Java folder field; both are the prefs above.

`cargo test -p bhpc-mods --features jvm` runs a Java client mod through `ClientHost` (needs a JDK and a JRE); CI has a
job for it.

## Native client mods (milestone M10)

A native mod is an armv7 `client/native/armeabi-v7a/*.so` in a package with `permissions.native` (API and rules:
mod-sdk `docs/native.md`). The runner loads it into the **guest**: the library is linked like the game's own libraries
(its imports bind to the host's bionic: libc, libm, liblog), its constructors run, then `bhmod_native_init(api)` runs on
the thread that starts the session. The `bhmod_api_t` table lives in guest memory and its functions are host thunks
(`bhpc-run/src/native.rs`). Hooks are installed by name through `bhmod-native`'s engine over a `Host` on guest memory:

- code patches (the 8-byte entry patch, the trampoline) = write the guest page (made writable for the moment, the
  page protection of the segment is restored), then `Cpu::invalidate` for the JIT;
- trampolines live in a 64 KiB executable arena in the guest;
- modules and symbols come from a snapshot of the linker's modules (`Module::dynamic` gives the dynamic section, so
  imports can be found); Objective-C swizzles call the game's own `class_getInstanceMethod` /
  `method_setImplementation` in the guest;
- the mod's handlers are guest code: the game calls them directly, no host code in between.

A mod's library is loaded when its session starts, only if the package's grant (`mods.toml`) includes `native` and
"never allow native mods" is off (`--mods-block-native`); the grant is made by the launcher's install dialog (both native
boxes) or the join prompt (index-listed file, danger zone, confirmation). `bhmod_native_shutdown` runs and the hooks are
undone when the session ends. Libraries stay linked in the guest; the next session calls `init` again on the same image.

Weapon skins are a user of the same engine (`weapons.rs`, the module doc has the details): an inline hook on the
exported `texCoordsForItemType`, active while a mod declares a weapon cosmetic. The swap is scoped to the held item's draw (the
game's atlas is bound back right after it, so the dig-target indicator and the HUD show the normal icons). Dev aids: `BHPC_DEV_COSMETICS=weapon=<key>`
with `BHPC_DEV_WEAPON_ITEM=<item type>` (the flint spade is 16), `BHPC_WEAPONS_TRACE=1` (which sprite items the game draws
from where).

Mod item names: both the singular and the plural form of the game's item-name function are hooked (`items.rs`: twelve
singular and ten plural copies, 1.7.6 prologue checked); the plural uses the item's `plural` text, else its singular.

Mod items (content mods with `[content] items`, `items.rs`, see mod-sdk `docs/content.md`) use the same engine with
owner `u32::MAX - 2`: the twelve compile-unit copies of the game's static `NSStringFromItemType` (offsets in the 1.7.6
build, prologue checked) answer an `NSString` for a mod item's name, and the exported `itemTypeIsValidFillItem` says yes
for mod item ids so the inventory accepts them. Installed while a loaded mod declares an item; `scripts/e2e-content.sh`
runs the real game against the `decor-items` example (screenshots show names and icons).

Tests: `tests_native.rs` runs the real fixture libraries (`fixtures/native`, own C, build output committed; rebuild
with `fixtures/native/build.sh`, NDK needed) on the JIT against a fake game; `scripts/e2e-native.sh` runs the example mod
of mod-sdk (`scripts/build-native-example.sh`) and the weapon skin on the real game, with a pixel check of the skin.

## Voice chat, overhead icons, key bindings and the settings screen (API 0.3.4)

The voice core is mod-sdk's `bhmod-voice` `VoiceClient` (key exchange, crypto, Opus via `bhmod-opus-sys`, VAD,
push-to-talk state machine, jitter buffers, the mix). The PC side is thin and lives in covered crates; the play glue
(`modglue.rs`, `play.rs`, `cosmetics.rs`) only connects them:

| Piece | Where | Notes |
| --- | --- | --- |
| HELLO features | `bhpc_mods::voice::features(voice)` -> `JoinConfig.features` -> `session::Options.features` | `settings` always; `voice` when this build has a voice client (`modglue::voice_client`). (`voice-unsequenced` is retired: voice frames use only the server's voice UDP port.) |
| Voice ops | `JoinFlow` -> `Action::Voice` / `SettingsSchema` / `SettingsResult` | Only when both sides listed the feature (`Session::voice_enabled` / `settings_enabled`). |
| Driver seam | `bhpc_mods::voice::VoiceDriver` (+ `VoiceHost` via `as_host`) | The calls the loader makes on the voice client; `VoicePump` drives it once per frame from a `VoiceIo`. |
| Devices | `bhpc_run::voicenet::PcVoiceIo` | `MicControl` (mic), `VoiceOut` stream `MIX_STREAM` (the client's finished stereo mix, ~60 ms queued), reliable packets for the glue (0xBC via `JoinFlow::voice_packet`), a UDP socket to `server_ip:udp_port` (the voice port from `VOICE_KEY`, by default the game port + 1): the only path of voice frames (`AUTH` first, frames after `AUTH_OK`). A server without a voice port leaves voice off (`voice:mic` reports `unavailable`). |
| Microphone opt-in | `VoicePump` + `ModUi` mic dialog, `<data>/voice.toml` (`VoicePrefs`) | Off until a mod calls `voice.set-mic(true)`; the first time the player is asked (Allow / Not now); the answer, the client's volumes/mutes and the launcher master volume last applied are kept on the device. `voice:mic` reports `on` / `off` / `denied` / `unavailable`. |
| Push-to-talk | `TalkKeys` (`KeyIn::Talk`) -> `VoicePump::talk` -> `voice.talk(mode)` | The loader's own V/B/G/J keys; skipped for a key a mod bound itself. |
| Key bindings | `bhpc_mods::binds::Binds`, `<data>/mod-keys.toml` | `input.bind` / `unbind` / `key-of`; the window posts every named key (`bhpc_input::keymap::api_key_name`, not while typing) as `KeyIn::Key`; `ClientHost::key` fires `<mod>:input`. Rebinding in the settings screen. |
| Overhead icons | `bhpc_mods::overhead::Overheads`, `cosmetics::note_position`, `overlay::PadView::overheads`, `hud::overhead_picture` | One icon per (mod, player), the most important wins, the admin's `icons` filter. The blockhead draw hook (t0) records the head (`DynamicObject.floatPos` + 2.6 tiles, owner by `Blockhead.clientName`, own blockhead = local alias); the window maps it with the game's tap matrices (`gameui::grid`) into sprites. |
| Settings screen | `UiCommand::OpenSettings` -> `Effect::OpenSettings` -> `bhpc_mods::form::SettingsView` drawn by `ModUi` | Rows: voice (volume, microphone), the mod's `scope = "client"` settings (stored in `<data>/mod-settings.toml` through `HostOptions::client_settings`), its keys, and for admins the server settings from `SETTINGS_SCHEMA` (changes go out as `SETTINGS_SET`, the answer shows in the footer). `bool` / `int` / `float` / `enum` change here; `string`, `roles`, `matrix`, `list` are shown only. Keys: arrows, Enter, Esc; clicks. |
| HUD | `bhpc_mods::hud::Hud` (bridge), `ui/hud.rs` (drawing), `View.widgets/notices/dialog/mod_toasts` | Widgets (text, counter, panel; top-left / top-right; 16 per mod; the player hides a mod's widgets in its settings screen, kept in `<data>/mod-hidden-widgets.txt`), toasts (4 s), notices (levels, top centre, 10 s, 3 at once), dialogs (1-4 buttons, optional text field; Enter / Esc / arrows / Tab / clicks; typed characters come from SDL text input; one at a time; the answer is `<mod>:ui-action`, never blocking the game thread). |
| On-screen buttons | `UiCommand::SetButton` / `RemoveButton` | `unsupported` on desktop (`ui.has-touch` is false): desktop mods bind a key (`input.bind`). |
| Per-player volume | settings screen section "Players' voices" | Every speaker of the voice client (`VoiceClient::speakers`) with alias, volume 0-200 % and local mute; the voice mod's click-on-a-blockhead dialog uses the same calls (`voice.set-player-volume`, `mute-player`). |
| Player ids | `ClientHost::set_ids`, `Bridge::id_of` | Players are named by their vanilla `playerID` wherever it is known (`playerID` of the client's player list, `localPlayerID`, `VOICE_STATE`), in events, chat, commands, positions and `players.*`; by alias before that. |
| Skipped server mods | `Action::Skipped` -> warning notice at the session start | "This server has Voice (voice chat): update Blockheads PC to use it" in the launcher's language (`--lang`). |
| Launcher | Settings > Audio > Voice chat | Voice volume (`--voice-master`, applied at the next session when it changed) and the microphone (`--mic-device`, list from `bhpc-run --list-mics`). |

Lower seams from the V5 groundwork:

| Seam | Where | Notes |
| --- | --- | --- |
| Microphone | `bhpc_audio::capture` (`MicControl` shared, `MicDriver` on the window thread), `SdlBackend::open_capture` | 48 kHz mono `i16`, 20 ms (960-sample) chunks, bounded non-blocking queue (50). Off by default, open only while `set_wanted(true)`; the glue forces it off outside a server's world. |
| Mic-on indicator | `overlay::PadView::mic` + `hud::mic_picture` | Drawn by the host top-right whenever the device is open (`MicControl::live`); mods cannot reach it. |
| Playback | `bhpc_audio::voice::VoiceOut` | Per-speaker streams (`push`, gain and pan baked in) or the finished mix (`push_stereo`). Cleared outside a server's world. |

`BHPC_VOICE_TRACE=1` logs push-to-talk keys.

## Newer mod loader: "Update after this game"

When a server needs a newer loader or API than this build, the check refuses the package before anything runs and the
join fails with the "Update needed" notice; the connection is closed like for every failure, so there is no "join now
with what we have". The button "Update after this game" (Enter) is the smallest honest version of the design's: the game
cannot update the launcher, so it writes a note (`<data>/mods-cache/update-after-game`, `ModsDirs::request_update`) and
returns to the server list. The next time the launcher starts it reads and removes the note (`Boot.update_requested`)
and runs the update check even if the automatic check is off; the update itself stays a notice the player confirms
(the launcher never updates by itself).
