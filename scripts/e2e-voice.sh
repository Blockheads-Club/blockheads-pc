#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# End-to-end test of voice chat (API 0.3.4): a local blockheads-server with the official voice mod (permission
# `voice` granted, the voice UDP port = game port + 1) and three headless games on the user's own APK:
#
#   ALICE  joins through the game's Join form, answers the mod prompt with Enter, talks: test microphone `pulse`
#          (a 440 Hz tone 300 ms of every second), voice activation.
#   BOB    joins the same way with --mods-auto-accept, talks too (a continuous 660 Hz tone).
#   VANILLA joins with the loader off (--no-mods) and must get in silently.
#
# Checked: both clients get AUTH_OK on the voice UDP port, frames of each reach the other (decoded, audible mix),
# `/mods set voice mode_proximity false` on the server console stops the frames live and `true` brings them back,
# `/voice mute ALICE` stops ALICE's audio at BOB and `/voice unmute` brings it back, overhead icons (speaking, admin
# mute, no voice) and talk events show in the traces. The test hooks: BHPC_VOICE_TEST_MIC (bhpc_audio::testmic),
# BHPC_VOICE_TRACE (voice path counters, onsets for latency). Each game has its own data folder and so its own device
# id (<data>/device-id): a server lets one device play under one name only.
#
# Needs: the APK (as smoke-game.sh), xvfb-run, dbus-run-session, a blockheads-server with voice
# ($BHPC_SERVER_BIN) and the voice mod folder ($BHPC_VOICE_MOD: mod.toml, server/mod.wasm, client/mod.wasm, as
# mod-sdk examples/voice-rust after scripts/build-wasm.sh). Without them the test is SKIPPED.
# Usage: scripts/e2e-voice.sh [--no-build]   (env: BHPC_E2E_PORT, default 15600; KEEP=1 keeps the run dir)
set -uo pipefail
cd "$(dirname "$0")/.."

build=1
[[ "${1:-}" == "--no-build" ]] && build=0
apk="${BHPC_APK:-}"
for c in "$HOME/.local/share/blockheads-pc/game/TheBlockheads.apk" "$HOME/Downloads/The Blockheads_1.7.6.apk"; do
  [[ -z "$apk" && -f "$c" ]] && apk="$c"
done
server="${BHPC_SERVER_BIN:-}"
mod="${BHPC_VOICE_MOD:-}"
if [[ -z "$apk" || ! -f "$apk" || ! -x "$server" || ! -f "$mod/mod.toml" ]]; then
  echo "e2e-voice: SKIPPED (needs the game APK, BHPC_SERVER_BIN and BHPC_VOICE_MOD)"
  exit 0
fi
if ((build)); then
  cargo build --release -p bhpc-run --features play || exit 1
fi
bin="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/release/bhpc-run"
port="${BHPC_E2E_PORT:-15600}"

run="$(mktemp -d "${TMPDIR:-/tmp}/bhpc-e2e-voice.XXXXXX")"
pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2> /dev/null || true; done
  exec 9>&- 2> /dev/null
  [[ "${KEEP:-0}" == 1 ]] || rm -rf "$run"
}
trap cleanup EXIT
note() { echo "$(date +%s%3N) $*" >> "$run/timeline.log"; echo "e2e-voice: $*"; }

mkdir -p "$run/mods/voice" "$run/server-data"
cp -r "$mod"/. "$run/mods/voice/"
printf '[voice]\ngrant = ["voice"]\n' > "$run/mods/mods.toml"
mkfifo "$run/server.in"
BLOCKHEADS_DATA_DIR="$run/server-data" BLOCKHEADS_MODS_DIR="$run/mods" \
  "$server" --new e2evoice --port "$port" < "$run/server.in" > "$run/server.log" 2>&1 &
pids+=($!)
exec 9> "$run/server.in"
for _ in $(seq 60); do grep -q "World load complete" "$run/server.log" && break; sleep 0.5; done
console() { note "console: $*"; echo "$*" >&9; }

# The socket path of the private bus must stay short: tmpdir=/tmp.
cat > "$run/bus.conf" << EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF

# A fresh game: consent, first world, back to the menu, Join World > Advanced > Online IP/Port, port and nickname,
# Join; the mod prompt ($2), the welcome notice, then the portal: warp in a blockhead ($4 ms later, so the first
# player is off the portal), then $5 (a walk), stay $3 ms. Coordinates are for the default 1024x768 window.
script() {
  cat << EOF
0     clicktext Allow
+0    clicktext OK
+1500 click 20 20
+1500 click 512 282
+8000 click 712 130
+4000 click 512 360
+3000 click 512 360
+3000 click 580 408
+2000 key backspace
+150  key backspace
+150  key backspace
+150  key backspace
+150  key backspace
+300  text $port
+500  key enter
+1500 click 580 456
+2000 key backspace
$(for _ in $(seq 10); do echo "+150  key backspace"; done)
+300  text $1
+500  key enter
+1500 click 512 552
+4000 shot prompt
+200  $2
+1000 clicktext OK
+3000 wait
+${4} wait
+200  click 512 400
+3000 click 512 490
+6000 shot spawned
+200  $5
+$3   shot late
+500  quit
EOF
}
# game NAME SCRIPT ENV... -- ARGS...
game() {
  local name=$1 script=$2
  shift 2
  local envs=()
  while [[ $# -gt 0 && $1 != -- ]]; do envs+=("$1"); shift; done
  shift
  mkdir -p "$run/$name/home" "$run/$name/data" "$run/$name/shots"
  env -u WAYLAND_DISPLAY -u DISPLAY HOME="$run/$name/home" BROWSER=true SDL_AUDIODRIVER=dummy SDL_VIDEODRIVER=x11 \
    GDK_BACKEND=x11 "${envs[@]}" timeout --kill-after=10 420 dbus-run-session --config-file="$run/bus.conf" -- \
    xvfb-run -a -s "-screen 0 1280x800x24" "$bin" --apk "$apk" --data-dir "$run/$name/data" --script "$script" \
    --shot-dir "$run/$name/shots" --seconds 400 "$@" > "$run/$name.out" 2> "$run/$name.err" &
  pids+=($!)
}
wait_for() { # file pattern seconds
  for _ in $(seq $(($3 * 2))); do grep -q -- "$2" "$1" 2> /dev/null && return 0; sleep 0.5; done
  return 1
}

for n in alice bob; do
  mkdir -p "$run/$n/data"
  # The microphone answer of the loader and the voice mod's client settings: mic on, voice activation.
  printf 'mic = "granted"\n' > "$run/$n/data/voice.toml"
  printf '[voice.settings]\nmic = "on"\nactivation = "vad"\n' > "$run/$n/data/mod-settings.toml"
done
script ALICE "key enter" 200000 0 "click 700 380" > "$run/alice.script"
script BOB wait 190000 15000 "click 330 380" > "$run/bob.script"
script VANILLA wait 30000 0 wait > "$run/vanilla.script"
game alice "$run/alice.script" BHPC_VOICE_TRACE=1 BHPC_VOICE_TEST_MIC=pulse:440:300:1000 --
sleep 8
game bob "$run/bob.script" BHPC_VOICE_TRACE=1 BHPC_VOICE_TEST_MIC=sine:660 -- \
  --mods-auto-accept
wait_for "$run/bob.err" "got auth_ok [1-9]" 300 || note "BOB never got AUTH_OK"
for _ in $(seq 120); do [[ -s "$run/bob/shots/spawned.png" ]] && break; sleep 1; done
sleep 5
note "phase baseline"
sleep 20
console "/mods set voice mode_proximity false"
sleep 15
console "/mods set voice mode_proximity true"
sleep 15
console "/voice mute ALICE"
sleep 15
console "/voice unmute ALICE"
sleep 15
note "phase vanilla"
game vanilla "$run/vanilla.script" -- --no-mods
for _ in $(seq 150); do [[ $(grep -c "Player Connected" "$run/server.log") -ge 3 ]] && break; sleep 1; done
note "end"

fail=0
expect() { # FILE PATTERN WHAT
  if grep -qE -- "$2" "$1"; then echo "  ok: $3"; else echo "  FAILED: $3 (no /$2/ in $(basename "$1"))" >&2; fail=1; fi
}
expect "$run/alice.err" "the server uses mods: asking the player" "ALICE: join prompt"
expect "$run/alice.err" "voice chat is on for this server" "ALICE: voice session"
expect "$run/bob.err" "voice chat is on for this server" "BOB: voice session (auto-accept)"
expect "$run/alice.err" "got auth_ok [1-9]" "ALICE: AUTH_OK on the voice UDP port"
expect "$run/bob.err" "got auth_ok [1-9]" "BOB: AUTH_OK on the voice UDP port"
expect "$run/bob.err" "event [0-9]+ TalkStart" "BOB: talk-start of ALICE"
expect "$run/bob.err" "overhead icons .*Speaking" "BOB: speaking icon"
expect "$run/bob.err" "overhead icons .*AdminMute" "BOB: admin mute icon"
expect "$run/bob.err" "overhead icons .*NoVoice" "BOB: no-voice icon of the vanilla player"
expect "$run/server.log" "Player Connected VANILLA" "VANILLA joined with the loader off"
ids=$(cat "$run"/{alice,bob,vanilla}/data/device-id 2> /dev/null | sort -u | grep -cE '^[0-9a-f]{16}$')
[[ $ids == 3 ]] && echo "  ok: three device ids" || { echo "  FAILED: $ids distinct device ids" >&2; fail=1; }
[[ $(grep -c '\[mods\]' "$run/vanilla.err") == 0 ]] && echo "  ok: VANILLA ran no mod code" || {
  echo "  FAILED: the vanilla game logged mod lines" >&2
  fail=1
}
# Frames by phase from the timestamped counters: what each side received while the other talked.
python3 -I - "$run" << 'PY' || fail=1
import re, sys
run = sys.argv[1]
tl = {w: int(t) for t, w in re.findall(r'^(\d+) (.*)$', open(run + '/timeline.log').read(), re.M)}
def stats(f):
    return [tuple(map(int, m)) for m in re.findall(
        r'stats (\d+) mic \d+/\d+ audible, sent auth \d+ frames (\d+), got auth_ok \d+ frames (\d+) other \d+, '
        r'mix (\d+)/\d+ audible', open(f).read())]
def at(st, t):
    p = [s for s in st if s[0] <= t]
    return p[-1] if p else (0, 0, 0, 0)
A, B = stats(run + '/alice.err'), stats(run + '/bob.err')
keys = ['phase baseline', 'console: /mods set voice mode_proximity false', 'console: /mods set voice mode_proximity true',
        'console: /voice mute ALICE', 'console: /voice unmute ALICE', 'phase vanilla']
names = ['baseline', 'proximity off', 'proximity on', 'ALICE muted', 'ALICE unmuted']
ok = True
for i, n in enumerate(names):
    s, e = tl[keys[i]], tl[keys[i + 1]]
    a0, a1, b0, b1 = at(A, s), at(A, e), at(B, s), at(B, e)
    sent_a, got_b, aud_b = a1[1] - a0[1], b1[2] - b0[2], b1[3] - b0[3]
    sent_b, got_a, aud_a = b1[1] - b0[1], a1[2] - a0[2], a1[3] - a0[3]
    print('  %-14s ALICE sent %4d -> BOB got %4d (audible mixes %4d) | BOB sent %4d -> ALICE got %4d (audible %4d)'
          % (n, sent_a, got_b, aud_b, sent_b, got_a, aud_a))
    heard = got_b > 100 and aud_b > 50
    want = n not in ('proximity off', 'ALICE muted')
    if heard != want:
        print('  FAILED: %s: BOB %s ALICE' % (n, 'heard' if heard else 'did not hear'))
        ok = False
mic = [int(x) for x in re.findall(r'onset mic (\d+)', open(run + '/alice.err').read())]
mix = [int(x) for x in re.findall(r'onset mix (\d+)', open(run + '/bob.err').read())]
lat = sorted(t - max(m for m in mic if m <= t) for t in mix if any(m <= t for m in mic))
lat = [x for x in lat if x < 1000]
if lat:
    print('  latency ALICE mic onset -> BOB mix onset: n=%d p50=%d p90=%d ms' % (len(lat), lat[len(lat) // 2],
                                                                              lat[int(len(lat) * 0.9)]))
sys.exit(0 if ok else 1)
PY
if ((fail)); then
  KEEP=1
  echo "e2e-voice: FAILED; run dir kept: $run" >&2
  exit 1
fi
echo "e2e-voice: OK"
