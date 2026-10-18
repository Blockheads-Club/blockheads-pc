# Blockheads PC launcher

`app/` is the desktop launcher (Tauri 2, SvelteKit with Svelte 5, TypeScript, bun), laid out like the sibling apps
(`src/lib/{ui,screens,layout,parts}`, `messages.ts`, `i18n.svelte.ts`, `theme.svelte.ts`, `api.ts`, `mock-api.ts`,
fontsource fonts bundled locally, no network at run time). The visual design is the "Sunlit Blocks" system with the
PC extension (dark play stage, setup rail, key caps).

All logic is in `crates/bhpc-launcher` (100 % line and function coverage in `scripts/coverage.sh`);
`app/src-tauri/src/lib.rs` only converts types. The interface is covered by vitest at 100 % statements, branches,
functions and lines, and by Playwright against a mock backend.

## Layout

| Path | What |
|---|---|
| `crates/bhpc-launcher` | APK check (4 error kinds), `LaunchConfig` TOML, key bindings with conflicts, window options, save import, `ops` (what each command does), `progress` (start-up progress protocol, weights, estimate), `session` (spawn/stop/poll `bhpc-run`, bounded log, stall guard) |
| `app/src/lib` | `store.svelte.ts` (state), `api.ts` (Tauri commands), `mock-api.ts` (test fake), `screens/`, `parts/`, `ui/`, `layout/` |
| `app/src-tauri` | Tauri shell, capabilities, bundle config, icons, sidecar (`binaries/`, built, not committed) |
| `app/e2e` | Playwright specs and the in-page mock backend |

Files the launcher writes (per user): `launcher.toml` (game config) and `prefs.toml` (language, theme, ...) in the
config dir (`~/.config/blockheads-pc`, `%APPDATA%\blockheads-pc`, `~/Library/Application Support/blockheads-pc`),
overridable with `BHPC_CONFIG_DIR`; the data dir (`BHPC_DATA_DIR` for the default) holds worlds and a private copy of
the APK (`game/TheBlockheads.apk`) when "Keep a private copy" is on.

`bhpc-run --config launcher.toml` expands to `--apk --data-dir --window WxH [--force] [--fullscreen] [--no-vsync]`
(`bhpc_launcher::config::expand_args`); flags given next to `--config` come first and win. `BHPC_RUN=/path` makes the
launcher start another `bhpc-run` build (development).

## Develop

```sh
cd app
bun install
bun run dev            # the UI in a browser, needs the Tauri IPC: use the e2e mock or `bun run tauri dev`
bun run check          # svelte-check
bun run test           # vitest with the 100 % coverage gate
bun run test:e2e       # Playwright against the built UI and the mock backend
SHOT_DIR=/tmp/shots bunx playwright test e2e/shots.spec.ts   # screenshots of every screen, light and dark
bun run tauri dev                                            # real backend; refreshes the sidecar first (see below)
```

## The sidecar is always rebuilt from the current sources

A launcher build once bundled a stale `bhpc-run`. `tauri.conf.json` therefore runs `scripts/ensure-sidecar.mjs` as
`beforeDevCommand` and `beforeBuildCommand`; it calls `scripts/build-sidecar.sh`, an incremental `cargo build` (a few
seconds when nothing changed) that copies the result to `app/src-tauri/binaries/`. So `bun run tauri dev` and
`bun run tauri build [--no-bundle]` can not ship a runner older than `crates/`. `BHPC_SKIP_SIDECAR=1` skips the
step; CI sets it because it builds the sidecar for the target triple in the step before (`launcher-bundles.yml`).
The first build after a clean checkout compiles static SDL2 and dynarmic and takes minutes.

## Installers

`scripts/build-sidecar.sh [triple]` builds `bhpc-run` (play build, static SDL2) into
`app/src-tauri/binaries/bhpc-run-<triple>`; Tauri's `externalBin` ships it next to the launcher executable. Then
`cd app && bun run tauri build` makes the bundles of the host OS: Linux AppImage, deb and rpm; Windows NSIS and MSI;
macOS `.app` and dmg. `.github/workflows/launcher-bundles.yml` does this on all three. Bundles contain the launcher,
the sidecar and icons only: no APK, game libraries or assets. Product name "Blockheads PC", identifier
`club.blockheads.pc`, licence GPL-3.0-only. `scripts/gen-notices.mjs` regenerates
`app/static/third-party-notices.txt` (shown under Settings > Updates & about).

macOS: the sidecar needs the JIT entitlements (`app/src-tauri/entitlements.plist`); signing and notarisation are not
configured. Windows and macOS bundles have never been built (no runners yet).

## Playing in the launcher window

The requirement: the game must not open in a separate window; the launcher's frameless window (title bar, control
strip) shows it below the strip, and fullscreen hides the bars.

Architectures considered:

* **(a) In-process** (launcher links `bhpc-run`, renders into a GtkGLArea / child HWND / NSView): rejected for now. The
  guest main thread, the UI thread and the GL contexts are bound to the SDL window, a JIT crash would take the
  launcher with it, and each OS needs its own context glue.
* **(b) Out-of-process with frame sharing**: chosen. Cross-process window reparenting does not exist on Wayland, but
  sharing pixels works everywhere and keeps crash isolation (a crash is a "crashed" state with logs, as before).

How it works on Linux (X11 and Wayland, same code through GTK):

1. `game_start` creates a **frame file** (`$XDG_RUNTIME_DIR/bhpc-frame-<pid>-<n>`, fixed size, three BGRA slots, header
   with a sequence number; `bhpc_launcher::embed`) and starts `bhpc-run --embed FILE` with stdin piped.
2. The runner forces SDL's *offscreen* video driver (no window; audio is unaffected), waits for the launcher's first
   `size`, renders as usual and, once per presented frame (at most 60 per second), composes the letterboxed game plus
   the Java-view overlay into a texture of exactly the view size, reads it back and publishes it into the frame file.
   (The offscreen driver does not resize its surface with the window, so the window's own framebuffer is not used.)
3. The launcher adds a GTK `DrawingArea` as an **overlay over the webview** (`app/src-tauri/src/embed.rs`), below the
   title bar and strip (the page reports where the stage starts, `embed_layout`; `-1` hides the view while a dialog is
   open; `0` is fullscreen). Once per display frame it copies the newest frame into a cairo surface and paints it.
4. Mouse, wheel, keys (GDK keycode to HID usage), text (GTK input method), focus and size go to the runner as text
   lines on its stdin (`embed::Cmd`). The runner exits when stdin closes, so it never outlives the launcher.
   Frame files of dead launchers are deleted at the next start.
5. If the runner cannot get a GL context without a window it exits with code 3 and the launcher restarts it with a
   window of its own; so does a platform without the view. `BHPC_EMBED=0` forces the separate window.

The runner logs `[embed] frame path: shared frame file, WxH, N fps, X ms per frame` every ten seconds.
Headless measurement (Xvfb, llvmpipe software GL, so the read-back also waits for the software rendering of the
compose pass; the game itself runs about 30 fps there): 1100x700 5 ms per frame, runner about 67 % of one core;
1920x1080 11 ms per frame, about 81 % of one core.

Not done yet (next step): a zero-copy GPU path. Export the composed texture as a dmabuf (`EGL_MESA_image_dma_buf_export`),
pass the fds over a unix socket (SCM_RIGHTS) and import them in the launcher (`EGL_EXT_image_dma_buf_import`) into a
GtkGLArea, with the shared-memory path as the automatic fallback. GTK3 uses GLX on X11, so this only helps on Wayland
(EGL), and it needs a GPU render node to test; that is why it is not part of this change. Windows (shared D3D/GL
handle into a child HWND) and macOS (IOSurface into an NSView) are also still to do; both fall back to a separate
window now.

## Start-up progress and the loading screen

While the game boots (about 5 to 30 s) the game area shows a loading screen with a real percentage. Design:
`PC-Loading` (screen) and `PC-Loading-Spec` (behaviour).

**Protocol.** `bhpc-run` prints these lines on stdout when started with `--embed` or `--progress` (the launcher passes
`--progress` always, so the separate-window fallback has the same loading state). A plain `bhpc-run --apk X` prints
nothing extra. Parsed by `bhpc_launcher::progress::parse_line`; they are kept out of the log.

```text
@progress <phase> <done> <total>    total 0 = no measurable steps (the bar eases within the step)
@ready                              the first frame was presented
```

| # | phase | what is counted | where |
|---|---|---|---|
| 1 | `apk` | reading and checking the APK (0/1, 1/1) | `apkrun::Session::open_setup` |
| 2 | `libraries` | mapping and relocating the libraries (0/0, 1/1) | same |
| 3 | `constructors` | every constructor, N/237 for the 1.7.6 build | `Session::run_constructors` (`Counting` trace) |
| 4 | `runtime` | Java start-up stages (`LibraryInit` ... `SurfaceInit`), N/6 | `play.rs` UI thread |
| 5 | `launch` | `nativeRun` (`UIApplicationMain`) and the four lifecycle stages, N/5 | `play.rs` |
| 6 | `frame` | waiting for the first presented frame (0/0), then `@ready` | `Hooks::before_swap` |

**Percentage.** `progress::Tracker` turns the last line into `base(step) + weight(step) * fraction`; steps without
counts (`total` 0) fill along `1 - e^(-t/usual)` up to 95 % of their share, so the bar keeps moving. The value never
decreases. The default weights are the design's (10, 18, 22, 22, 20, 8 %); after each start the step durations are
appended to `boot-times.toml` in the config dir and the weights become the average share of the last 5 starts (floor
2 % per step), so the bar becomes roughly linear in time on this computer. Measured on the dev machine (Xvfb, software GL, 12 s to the first frame): apk 0.2 s, libraries 0.4 s, constructors
2.5 s, runtime 2.4 s, launch 0.0 s, first frame 6.7 s, so the learned weights differ a lot from the defaults.

**Rules** (all in `progress.rs`, constants documented there): time left only after 3 s and 15 % (the screen rounds it:
1 s steps below 10 s, 5 s steps above, "about 1 min" cap); a step is *slow* after twice its usual time but not sooner
than 4 s and at the latest after 15 s (striped bar, note, "Still working..."); no progress line for 60 s is a *stall*:
the session kills the runner and reports it as crashed at that step.

**Launcher states.** `GameState::Starting` (JSON `starting`, badge "Starting...") lasts until `@ready`; then `running`.
A runner that prints no progress (older build, `BHPC_RUN` override) counts as starting for 2.5 s. The snapshot carries
`progress` (percent, step, eta, slow, stalled, step times, ready), also after a crash (where it got to). The page polls
every 250 ms while starting. `Playing.svelte` shows `parts/LoadingView.svelte` after 400 ms of starting (a faster
start shows nothing), fades it out over 240 ms on ready (the badge changes at the same moment, focus moves to the
game), Esc or Cancel stop at once (no dialog, Home says stopped), and a crash before the first frame keeps the screen
with the error, the step, the last log lines, Try again, Copy log and Back to Home.
`LoadingView` is a single component fed by props (progress, version, failed, handover, fullscreen, log tail), so a
restyle does not touch the plumbing.

**In-window view.** Until the first frame the GTK view draws nothing and lets the mouse through (`GtkOverlay`
pass-through), so the page's loading screen is visible and Cancel works; the first frame fades in over 240 ms (no
fade when GTK animations are off), takes the mouse and the keyboard focus.

## Deviations from the design

- **Playing, "game inside the launcher".** One window on Linux (see "Playing in the launcher window"). On Windows
  and macOS the game still opens in a window of its own (title "Blockheads PC · Playing The Blockheads 1.7.x" and the
  PC icon) while the launcher shows the Playing screen. The design's tip pill over the stage is not drawn (the stage
  is a native view). The window is square (no rounded corners) while the game view is up.
- **Fullscreen, vsync, key bindings, window size, sound.** Applied by `bhpc-run` (`--fullscreen`, `--no-vsync`,
  `--bind ACTION=BINDING` for every non-default binding, `--window`, `--title`; sound off sets
  `SDL_AUDIODRIVER=dummy`). The bindings reach the `bhpc-input` mapper (`Bindings`): touch, pan, zoom, pinch, back,
  fullscreen. In the in-window view the launcher itself handles the fullscreen binding (it owns the window).
- **Mute in background** is shown disabled ("not available yet"); **Game sound** off starts the game with
  `SDL_AUDIODRIVER=dummy`.
- **Controls**: six actions (the backend has one "touch" action for tap and drag, not two).
- **Import**: "Keep both" is not offered (skip or replace per file); progress is a busy bar, not a percentage; results
  list what was written grouped by folder, not by world name.
- **Window chrome**: frameless (custom bar, drag region, native resize) on Windows and Linux; on macOS the native
  traffic lights are kept (overlay title bar). The Linux button order setting (`gtk-decoration-layout`) is not read;
  Windows 11 Snap layouts on the maximize button are not wired.
- **Languages**: English and Turkish only.
- **Update check** is UI only: the backend answers "not available" and the interface hides it
  (`updates_enabled: false`); the full flow (available, downloading, installing, ready, playing, error) is built and
  exercised against the mock. "Restart now" just closes the window.
- **Splash**: a separate small window, never part of the main window (both variants: yellow setup splash on the very
  first launch, dark launcher splash on later starts, plus the slow and error states with retry, open folder and
  quit). See "Splash window" below.
- **Display detection** shows the monitor size and scale; the refresh rate is not available through Tauri.

## Splash window

On start only a frameless, transparent, non-resizable 600x380 window (the design's splash card, centred, not in the
taskbar) is shown; the `main` window is created hidden (`tauri.conf.json`, so the Wayland/GTK title-bar warning fix
holds) and runs the boot work (app info, boot, display info, APK recheck). The splash route is `/splash`
(`src/routes/splash`, content `screens/Splash.svelte`) and has its own capability file
(`capabilities/splash.json`, event listening only; its buttons are app commands).

- The main window pushes its boot state (`loading`/`slow`/`error`, first run, version, language) with `splash_set`;
  the splash renders it (`splash-state` event, `splash_get` for what was set before it opened).
- When the boot is done the main window calls `main_ready`: Rust waits for the minimum splash time, shows and focuses
  `main`, and closes the splash. On a boot error the main window stays hidden; the splash shows the error and
  Retry emits `splash-retry` to the main window, Quit exits. Closing the splash before that also quits.
- **`BHPC_SPLASH_HOLD_MS`**: minimum splash time in milliseconds (default 0), read in Rust. For testing the splash:
  `BHPC_SPLASH_HOLD_MS=5000 blockheads-pc`.
- macOS: the transparent window needs the private API, so there the splash is a plain undecorated window with square
  corners (`square` in the splash page). Not tested (no Mac runner).
- If the splash window cannot be created the main window is shown directly.
