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
| `crates/bhpc-launcher` | APK check (4 error kinds), `LaunchConfig` TOML, key bindings with conflicts, window options, save import, `ops` (what each command does), `session` (spawn/stop/poll `bhpc-run`, bounded log) |
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
scripts/build-sidecar.sh && bun run tauri dev                # real backend (from the repo root: ../scripts)
```

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

## Deviations from the design

- **Playing, "game inside the launcher".** The game is its own SDL window. The launcher shows the running state on a
  dark Playing screen (Home, status, help, logs, Stop) and notices a crash or exit by itself. The game window does not
  yet have the PC icon or the title "Blockheads PC · Playing The Blockheads 1.7.x": that needs `crates/bhpc-run/src/play.rs`
  (title and icon on the SDL window), which was being edited by someone else.
- **Fullscreen, vsync, key bindings.** Saved in `launcher.toml` and passed as flags, but `bhpc-run` only applies the
  window size today (`play.rs` hard-wires fullscreen off and vsync off and the input mapper's bindings).
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
- **Splash**: the setup splash is shown on the very first launch while the backend loads; the launcher splash on later
  starts and for a slow or failed start (retry, open folder, quit).
- **Display detection** shows the monitor size and scale; the refresh rate is not available through Tauri.
