# Playing Blockheads PC

How to play once the game runs (from the launcher or `bhpc-run --apk ...`). Building from source:
[development.md](development.md). Launcher details: [launcher.md](launcher.md).

## Controls

The game is a touch game and the mouse is the finger (`bhpc-input`):

| Input | Effect |
|---|---|
| left button press, drag, release | one-finger touch (tap, drag, long press = hold) |
| right button drag | two-finger pan |
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
- The game picks its render size itself (for example 1024x768 in a 1280x960 window, 1334x750 in 1600x900) and the
  window scales that picture, keeping its aspect ratio (bars at the sides or top and bottom), also on high-DPI
  displays and in fullscreen. It chooses the size once at start-up: resizing the window afterwards scales and
  letterboxes the same picture instead of re-laying the game out.
- `bhpc-run --config` applies the launcher's APK, data directory, forced build and window size only; fullscreen, vsync
  and key bindings are saved by the launcher but not used yet (see `docs/launcher.md`).
- Gameplay has been exercised only briefly; expect rough edges and report what you find.
- Losing the window focus does not pause the game (it keeps running unfocused); minimising does, like
  backgrounding the app on a phone.
