# Playing Blockheads PC

How to play once the game runs (from the launcher or `bhpc-run --apk ...`). Building from source:
[development.md](development.md). Launcher details: [launcher.md](launcher.md).

## Controls

The game is a touch game and the mouse is the finger (`bhpc-input`):

| Input | Effect |
|---|---|
| left button press, drag, release | one-finger touch (tap, drag, long press = hold) |
| right button drag | two-finger pan |
| middle click on a blockhead | mods: pick that player (the voice chat opens its volume for them) |
| mouse wheel | two-finger pinch (zoom) around the cursor |
| Ctrl + left drag | pinch around the press point (right/up = spread) |
| Esc | Android Back |
| letters, digits, arrows, Enter, Backspace, Delete, Tab, Space, Home, End | Android key events; while a text field has focus they type into it instead |
| F11 | toggle fullscreen (the picture is scaled to the screen, bars keep its aspect ratio) |
| H | hide / show the game's own on-screen buttons (not while typing) |
| close window | quit |

**Game controller** (SDL GameController, hot-plug, the first one connected plays; off with `--no-controller`):

| Controller | Effect |
|---|---|
| left stick | walk: a finger held on the game's on-screen arrow pad, moved the stick's way |
| right stick | move the cursor (drawn over the game); it snaps to the block under it when released |
| D-pad | move the cursor one block (repeats while held); reaches every menu button |
| A | tap at the cursor; hold to dig or drag |
| B | Android Back |
| X | hide / show the game's buttons |
| Y | next zoom level (normal, far, farther, close) |
| LB / RB | zoom out / in one step |
| Start | open / close the pause menu |
| right stick press | cursor to the middle |

## Joining a server

Start a server first (for example `blockheads_server --new myworld --port 15160`), then in the
game: Join World > Advanced... > Online IP/Port..., tap the port field and enter the server port (the field is
pre-filled with 15151), optionally set a nickname, then Join Online World. Details and a script that does this are in
[`m7/net-bringup.md`](m7/net-bringup.md).

Each install has its own device id (`DIR/device-id`, 16 random hex digits made on the first start; servers tie a
player's name to the device). Release note: *PC now has its own device id per install; servers may see an existing PC
player as a new device once.* (Before, every copy of the port reported the same id, so two PC players could not be on
one server at the same time.) Deleting the file makes a new device; `BHPC_ANDROID_ID=<hex>` overrides it for tests.

## Where saves go

Into the data directory: `--data-dir DIR`, or by default `$XDG_DATA_HOME/blockheads-pc`
(`~/.local/share/blockheads-pc`). Worlds and settings are under `DIR/home`, caches under `DIR/cache`, temporary
files under `DIR/tmp`; they persist between runs. Delete the directory to start from scratch.

## Known issues

- The game itself has been played on x86_64 Linux; the other hosts pass the same build and test suites in CI (which
  has no game APK), so their first real sessions may still find host-specific problems.
- Only the 1.7.4, 1.7.5 and 1.7.6 builds are known to boot.
- Performance is that of a JIT plus host re-implementations: expect the frame rate to be well below a phone's on
  busy scenes, and the first seconds of a start to be slow.
- A fresh data directory starts in a world scene a few seconds after the menu in the sanity run (first-run flow of
  the game); this has not been investigated.
- Some Java services are still stubs and answer 0/null (notifications, news client, in-app purchases, ad and
  analytics SDKs); the end-of-run report lists the ones the game called.
- The on-screen keyboard is replaced by host text input.
- The game's screen follows the window's size and aspect (no bars). How big the world and interface are is the
  launcher's "Game zoom" (`--zoom`): `phone` (default) gives the game a phone's 375 points on the short side (667x375
  at 16:9, what the Android game gets on a 1080p phone), `close` the older, ~17 % bigger iPhone 5 size (568x320) and
  `tablet` an iPad's 768 points (more world, small interface, the game's iPad layout). A bigger window shows the same
  picture bigger; the zoom is chosen once at start-up (`crates/bhpc-run/src/aspect.rs`).
- The launcher's "Game UI scale" (80–130 % in 5 % steps, `--ui-scale F`, passed only when not 100 %) fine-tunes the
  zoom: the game gets the zoom's points divided by it, so 120 % draws everything 20 % bigger and 80 % smaller, with
  more world. It is chosen at start-up too and kept when the window is resized. The zoom picks the game's layout and
  the scale never switches it: the phone layout (phone and close zoom) stays between an iPhone 4's 480x320 points
  (smaller cuts the pause menu off) and a 414-point short side (from 415 the game uses its iPad layout, whose main
  menu does not fit that few points). So at 16:9 the phone zoom's 80 % stops at 736x414 points and 130 % at 568x320
  (the close zoom's size; the close zoom does not grow further there), and on 4:3 the phone zoom spans 552x414 to
  480x360. The tablet zoom (iPad layout) takes the whole range: at 130 % on 16:9 it still has 1050x591 points.
- `bhpc-run --config` applies the launcher's APK, data directory, forced build and window size only; fullscreen, vsync
  and key bindings are saved by the launcher but not used yet (see `docs/launcher.md`).
- Gameplay has been exercised only briefly; expect rough edges and report what you find.
- Losing the window focus does not pause the game (it keeps running unfocused); minimising does, like
  backgrounding the app on a phone.
