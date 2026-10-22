#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# End-to-end test of the client mod loader (milestone M6): the real game, headless, joins a local Blockheads Server
# that runs the client-required test mod fixtures/mods/echo (Luau, client + server, channel echo + chat):
#
#   1. modded server: the join prompt shows, Enter accepts, the mod downloads, the 0x1f carries bhcModLoader, the
#      client mod runs, a channel message goes to the server and back, the server's chat line reaches the client mod,
#      the F9 Mods panel opens. A second join uses the cache (no download).
#   2. vanilla server (no mods): HELLO gets no answer, after 1.5 s the game joins exactly like a vanilla client.
#
# Needs: the user's APK (as smoke-game.sh), xvfb-run, dbus-run-session, and a blockheads-server binary with mod support
# (dev since the 0xBC merge): $BHPC_SERVER_BIN, else built from $BHPC_SERVER_SRC (a checkout). Without either the
# test is SKIPPED. Everything runs in a temp dir (KEEP=1 keeps it); only processes started here are stopped.
#
# Usage: scripts/e2e-mods.sh [--no-build]     (env: BHPC_E2E_TIMEOUT seconds per game run, default 120)
set -euo pipefail
cd "$(dirname "$0")/.."

build=1
[[ "${1:-}" == "--no-build" ]] && build=0
limit="${BHPC_E2E_TIMEOUT:-120}"

apk="${BHPC_APK:-}"
for c in "$HOME/.local/share/blockheads-pc/game/TheBlockheads.apk" "$HOME/Downloads/The Blockheads_1.7.6.apk"; do
  [[ -z "$apk" && -f "$c" ]] && apk="$c"
done
server="${BHPC_SERVER_BIN:-}"
if [[ -z "$server" && -n "${BHPC_SERVER_SRC:-}" ]]; then
  (cd "$BHPC_SERVER_SRC" && cargo build --release -p blockheads_server --bin blockheads_server)
  server="$BHPC_SERVER_SRC/target/release/blockheads_server"
fi
if [[ -z "$apk" || ! -f "$apk" || -z "$server" || ! -x "$server" ]]; then
  echo "e2e-mods: SKIPPED (needs the game APK and BHPC_SERVER_BIN or BHPC_SERVER_SRC)"
  exit 0
fi
for tool in xvfb-run dbus-run-session; do
  command -v "$tool" > /dev/null || { echo "e2e-mods: $tool is missing" >&2; exit 1; }
done
if ((build)); then
  cargo build --release -p bhpc-run --features play
fi
bin="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/release/bhpc-run"

run="$(mktemp -d "${TMPDIR:-/tmp}/bhpc-e2e-mods.XXXXXX")"
pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2> /dev/null || true; done
  [[ "${KEEP:-0}" == 1 ]] || rm -rf "$run"
}
trap cleanup EXIT
port=$((20000 + RANDOM % 20000))

# A server on $port with the mods in $1 (an empty folder: vanilla). Its stdin stays open (EOF stops it).
start_server() {
  local mods=$1 name=$2
  mkdir -p "$run/$name-data"
  mkfifo "$run/$name.in"
  BLOCKHEADS_DATA_DIR="$run/$name-data" BLOCKHEADS_MODS_DIR="$mods" \
    "$server" --new "e2e$name" --port "$port" < "$run/$name.in" > "$run/$name.log" 2>&1 &
  pids+=($!)
  sleep 600 > "$run/$name.in" &
  pids+=($!)
  for _ in $(seq 60); do
    grep -q "World load complete" "$run/$name.log" && return 0
    sleep 0.5
  done
  echo "e2e-mods: the $name server did not start" >&2
  tail -n 20 "$run/$name.log" >&2
  return 1
}
stop_server() {
  for p in "${pids[@]}"; do kill "$p" 2> /dev/null || true; done
  pids=()
  sleep 1
}

cat > "$run/bus.conf" << EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$run</listen>
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF

# One headless game run: `game NAME SCRIPT [args]`; output in $run/NAME.{out,err}, shots in $run/shots-NAME.
game() {
  local name=$1 script=$2
  shift 2
  mkdir -p "$run/home" "$run/data" "$run/shots-$name"
  env -u WAYLAND_DISPLAY -u DISPLAY HOME="$run/home" BROWSER=true SDL_AUDIODRIVER=dummy SDL_VIDEODRIVER=x11 \
    GDK_BACKEND=x11 timeout --kill-after=10 "$((limit + 20))" dbus-run-session --config-file="$run/bus.conf" -- \
    xvfb-run -a -s "-screen 0 1280x800x24" "$bin" --apk "$apk" --data-dir "$run/data" --script "$script" \
    --shot-dir "$run/shots-$name" --seconds "$limit" "$@" > "$run/$name.out" 2> "$run/$name.err" || true
}

fail=0
expect() { # expect FILE PATTERN WHAT
  if grep -qF -- "$2" "$1"; then
    echo "  ok: $3"
  else
    echo "  FAILED: $3 (no \"$2\" in $(basename "$1"))" >&2
    fail=1
  fi
}

# Fresh data: consent, the first world, back to the main menu; then --join takes over once the menu settles. Enter
# answers the join prompt (the game's 0x1f is held meanwhile, so the exact time does not matter). The run leaves the
# server's world before it quits.
cat > "$run/modded.script" << 'EOF'
0     clicktext Allow
+0    clicktext OK
+1500 click 20 20
+1500 click 512 282
+9000 shot prompt
+200  key enter
+400  shot downloading
+20000 shot inworld
+500  key f9
+1500 shot panel
+500  key f9
+1000 click 20 20
+1500 click 512 282
+4000 shot menu
+500  quit
EOF
mkdir -p "$run/mods"
cp -r fixtures/mods/echo "$run/mods/echo"
printf '[echo]\ngrant = ["channels", "players:chat"]\n' > "$run/mods/mods.toml"

echo "e2e-mods: modded server (port $port)"
start_server "$run/mods" modded
game modded "$run/modded.script" --join "127.0.0.1:$port"
c="$run/modded.err"
s="$run/modded.log"
expect "$c" "server contact from 127.0.0.1:$port: HELLO" "HELLO after the server's first packet"
expect "$c" "holding the game's 0x1f" "the 0x1f is held while the player decides"
expect "$c" "the server uses mods: asking the player" "join prompt"
expect "$c" "sending the 0x1f with bhcModLoader" "accepted: READY and bhcModLoader"
expect "$c" "echo: echo client enabled" "the client mod runs"
expect "$c" "echo: client got pong:hello" "channel round trip (client side)"
expect "$c" "echo: chat seen: echo heard hello" "server chat reaches the client mod"
expect "$s" "server got hello" "channel round trip (server side)"
expect "$s" "Player Connected" "the server approved the join"
for shot in prompt inworld panel; do
  [[ -s "$run/shots-modded/$shot.png" ]] && echo "  ok: screenshot $shot" || { echo "  FAILED: no $shot.png" >&2; fail=1; }
done
blobs=$(find "$run/data/mods-cache/blobs" -name '*.bhmod' | wc -l)
[[ $blobs == 1 ]] && echo "  ok: the package is in the cache" || { echo "  FAILED: $blobs cached packages" >&2; fail=1; }

# Second join through the game's own Join World form (a fresh start): accepted from the cache, no download. The
# server stops by itself when its last player leaves, so a new one runs.
stop_server
start_server "$run/mods" modded2
cat > "$run/again.script" << EOF
6000  hold 817 100 200
+4000 hold 510 455 200
+4000 hold 510 553 200
+4000 hold 270 525 200
+2500 key backspace
+150  key backspace
+150  key backspace
+150  key backspace
+150  key backspace
+300  text $port
+500  key enter
+2000 hold 750 393 200
+2500 text TESTER
+500  key enter
+2000 hold 750 651 200
+4000 shot prompt
+200  key enter
+15000 shot inworld
+500  quit
EOF
game again "$run/again.script"
expect "$run/again.err" "sending the 0x1f with bhcModLoader" "second join accepted"
expect "$run/again.err" "echo: client got pong:hello" "second join runs the cached mod"
[[ $(find "$run/data/mods-cache/blobs" -name '*.part' | wc -l) == 0 ]] || { echo "  FAILED: partial files left" >&2; fail=1; }
stop_server

echo "e2e-mods: vanilla server (port $port)"
mkdir -p "$run/nomods"
start_server "$run/nomods" vanilla
cat > "$run/vanilla.script" << 'EOF'
2000  wait
+20000 shot inworld
+500  quit
EOF
game vanilla "$run/vanilla.script" --join "127.0.0.1:$port"
expect "$run/vanilla.err" "sending the 0x1f (vanilla server)" "no SERVER_HELLO: vanilla join after 1.5 s"
expect "$run/vanilla.log" "Player Connected" "the vanilla server approved the join"
stop_server

if ((fail)); then
  KEEP=1
  echo "e2e-mods: FAILED; run dir kept: $run" >&2
  exit 1
fi
echo "e2e-mods: OK"
