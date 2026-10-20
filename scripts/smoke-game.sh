#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Headless game smoke test (part of scripts/check-full.sh): boot -> world creation -> in the world -> main menu, once,
# under a strict time limit. Runs the play build (`--features play`, release) under Xvfb with its own D-Bus session,
# HOME and data dir, so it never touches the user's game data, display or running launcher.
#
# The APK is the user's own: $BHPC_APK, else the launcher's imported copy, else ~/Downloads/The Blockheads_1.7.6.apk.
# Without one the smoke test is SKIPPED (exit 0), unless BHPC_SMOKE_REQUIRED=1. Never commit or bundle the APK.
#
# Usage: scripts/smoke-game.sh [--no-build]     (env: BHPC_SMOKE_TIMEOUT seconds, default 150; KEEP=1 keeps the run dir)
# Needs: xvfb-run, dbus-run-session, the SDL2 dev package (system SDL for the play build).
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/lib-cache.sh
source scripts/lib-cache.sh

build=1
[[ "${1:-}" == "--no-build" ]] && build=0
limit="${BHPC_SMOKE_TIMEOUT:-150}"

apk="${BHPC_APK:-}"
for c in "$HOME/.local/share/blockheads-pc/game/TheBlockheads.apk" "$HOME/Downloads/The Blockheads_1.7.6.apk"; do
  [[ -z "$apk" && -f "$c" ]] && apk="$c"
done
if [[ -z "$apk" || ! -f "$apk" ]]; then
  if [[ "${BHPC_SMOKE_REQUIRED:-0}" == 1 ]]; then
    echo "smoke-game: no APK (set BHPC_APK)" >&2
    exit 1
  fi
  echo "smoke-game: SKIPPED (no game APK; set BHPC_APK to run it)"
  exit 0
fi
for tool in xvfb-run dbus-run-session; do
  command -v "$tool" > /dev/null || { echo "smoke-game: $tool is missing" >&2; exit 1; }
done

if ((build)); then
  cargo build --release -p bhpc-run --features play
fi
bin="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/release/bhpc-run"
[[ -x "$bin" ]] || { echo "smoke-game: $bin not built" >&2; exit 1; }

run="$(mktemp -d "${TMPDIR:-/tmp}/bhpc-smoke.XXXXXX")"
trap '[[ "${KEEP:-0}" == 1 ]] || rm -rf "$run"' EXIT
mkdir -p "$run/home" "$run/data" "$run/shots"

# On a fresh data dir the game first asks for consent (a system alert, drawn as a host dialog: "Allow" is clicked),
# then creates "My First World" by itself (the first-run flow has no world options, and it is already the cheapest
# world creation: ~10 s). The script waits for the in-world welcome popup, enters the world, then pauses and exits
# to the main menu; `quit` closes the game through its lifecycle (background, terminate). Times are ms after the first frame (`+N` = after
# the previous step); `clicktext` waits for its text. Coordinates are for the default 1024x768 window.
cat > "$run/smoke.script" << 'EOF'
0     clicktext Allow
+0    clicktext OK
+1500 mark world
+500  shot inworld
+300  click 20 20
+1500 click 512 282
+4000 mark menu
+0    shot menu
+300  quit
EOF

# A private session bus without service activation: otherwise desktop helpers (xdg-desktop-portal, ksecretd) are
# started inside it, keep running after the test and write into its HOME.
cat > "$run/bus.conf" << EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$run</listen>
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF

echo "smoke-game: $apk (limit ${limit} s)"
start=$SECONDS
set +e
env -u WAYLAND_DISPLAY -u DISPLAY HOME="$run/home" SDL_AUDIODRIVER=dummy SDL_VIDEODRIVER=x11 GDK_BACKEND=x11 \
  timeout --kill-after=10 "$limit" dbus-run-session --config-file="$run/bus.conf" -- xvfb-run -a -s "-screen 0 1280x800x24" \
  "$bin" --apk "$apk" --data-dir "$run/data" --script "$run/smoke.script" --shot-dir "$run/shots" \
  --seconds "$((limit - 10))" > "$run/out.log" 2> "$run/err.log"
rc=$?
set -e
took=$((SECONDS - start))

if [[ $rc == 0 ]] && grep -q 'found "OK"' "$run/err.log" && grep -q 'mark menu' "$run/err.log" &&
  grep -q 'closing: the game goes to the background' "$run/err.log" &&
  [[ -s "$run/shots/inworld.png" && -s "$run/shots/menu.png" ]]; then
  echo "smoke-game: OK, in the world after $(grep -o '[0-9]* ms: mark world' "$run/err.log" | cut -d' ' -f1) ms (${took} s total)"
  exit 0
fi
echo "smoke-game: FAILED (exit $rc after ${took} s); last lines of the log:" >&2
grep '^\[script\]' "$run/err.log" >&2 || true
tail -n 30 "$run/err.log" >&2
KEEP=1
echo "smoke-game: run dir kept: $run" >&2
exit 1
