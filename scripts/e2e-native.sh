#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# End-to-end test of native client mods (milestone M10): the real game, headless, single-player, with the example
# native mod native-hello (mod-sdk: scripts/build-native-example.sh) installed and granted `native`. Expected: the mod's
# library is loaded into the guest, its three hooks (an Objective-C swizzle, an inline hook on drawShaderQuad and an
# import hook on glBindTexture) are installed by the host and run, the world opens, and "never allow native mods"
# keeps all of that off.
#
# Needs: the user's APK (as smoke-game.sh), xvfb-run, dbus-run-session, a release build with `--features play`, and
# the package: $BHPC_NATIVE_MOD (default ../mod-sdk/target/native-examples/native-hello-0.1.0.bhmod). Without them
# the test is SKIPPED. Everything runs in a temp dir (KEEP=1 keeps it); only processes started here are stopped.
#
# Usage: scripts/e2e-native.sh        (env: BHPC_E2E_TIMEOUT seconds per run, default 120)
set -euo pipefail
cd "$(dirname "$0")/.."
limit="${BHPC_E2E_TIMEOUT:-120}"
apk="${BHPC_APK:-}"
for c in "$HOME/.local/share/blockheads-pc/game/TheBlockheads.apk" "$HOME/Downloads/The Blockheads_1.7.6.apk"; do
  [[ -z "$apk" && -f "$c" ]] && apk="$c"
done
pkg="${BHPC_NATIVE_MOD:-../mod-sdk/target/native-examples/native-hello-0.1.0.bhmod}"
bin="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/release/bhpc-run"
if [[ -z "$apk" || ! -f "$apk" || ! -f "$pkg" || ! -x "$bin" ]]; then
  echo "e2e-native: SKIPPED (needs the game APK, a release build with --features play and the native-hello package)"
  exit 0
fi
for tool in xvfb-run dbus-run-session; do
  command -v "$tool" > /dev/null || { echo "e2e-native: $tool is missing" >&2; exit 1; }
done
run="$(mktemp -d "${TMPDIR:-/tmp}/bhpc-e2e-native.XXXXXX")"
trap '[[ "${KEEP:-0}" == 1 ]] || rm -rf "$run"' EXIT
cat > "$run/bus.conf" << EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$run</listen>
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF
# Consent, the world's welcome notice, a few seconds in the world (the mod counts world updates and quads), the
# pause menu, the main menu, quit.
cat > "$run/world.script" << 'EOF'
0     clicktext Allow
+0    clicktext OK
+1500 click 20 20
+1500 click 512 282
+12000 shot inworld
+300  click 20 20
+1500 click 512 282
+4000 shot menu
+300  quit
EOF
mkdir -p "$run/home"

game() { # game NAME [args]: output in $run/NAME.err, shots in $run/shots-NAME
  local name=$1
  shift
  # A fresh data folder every time (consent and the first world are part of the script), with the mod granted.
  rm -rf "$run/data"
  mkdir -p "$run/shots-$name" "$run/data/mods"
  cp "$pkg" "$run/data/mods/native-hello.bhmod"
  printf '[native-hello]\ngrant = ["native"]\n' > "$run/data/mods/mods.toml"
  env -u WAYLAND_DISPLAY -u DISPLAY HOME="$run/home" SDL_AUDIODRIVER=dummy SDL_VIDEODRIVER=x11 GDK_BACKEND=x11 \
    timeout --kill-after=10 "$((limit + 20))" dbus-run-session --config-file="$run/bus.conf" -- \
    xvfb-run -a -s "-screen 0 1280x800x24" "$bin" --apk "$apk" --data-dir "$run/data" --script "$run/world.script" \
    --shot-dir "$run/shots-$name" --seconds "$limit" "$@" > "$run/$name.out" 2> "$run/$name.err" || true
}
fail=0
expect() { # expect FILE PATTERN WHAT
  if grep -qF -- "$2" "$1"; then echo "  ok: $3"; else echo "  FAILED: $3 (no \"$2\" in $(basename "$1"))" >&2; fail=1; fi
}
reject() { # reject FILE PATTERN WHAT
  if grep -qF -- "$2" "$1"; then echo "  FAILED: $3 (\"$2\" in $(basename "$1"))" >&2; fail=1; else echo "  ok: $3"; fi
}

echo "e2e-native: native mod on"
game on
c="$run/on.err"
expect "$c" "native code of native-hello loaded" "the library is loaded into the guest"
expect "$c" "[native-hello] hello from native code" "bhmod_native_init ran and logged through the host"
expect "$c" "world update: hooked" "Objective-C swizzle by selector"
expect "$c" "drawShaderQuad: hooked" "inline hook by exported symbol"
expect "$c" "glBindTexture: hooked" "import (GOT) hook"
expect "$c" "first world update through the hook" "the swizzle runs"
expect "$c" "first quad through the hook" "the inline hook runs (trampoline calls the original)"
expect "$c" "first texture bind through the hook" "the import hook runs"
[[ -s "$run/shots-on/inworld.png" ]] && echo "  ok: screenshot inworld (the game still draws)" || { echo "  FAILED: no inworld.png" >&2; fail=1; }
[[ -n "${SHOTS:-}" ]] && mkdir -p "$SHOTS" && cp "$run"/shots-on/*.png "$SHOTS/" || true

echo "e2e-native: never allow native mods"
game blocked --mods-block-native
c="$run/blocked.err"
reject "$c" "native code of native-hello loaded" "nothing is loaded"
reject "$c" "hello from native code" "the library did not run"
[[ -s "$run/shots-blocked/inworld.png" ]] && echo "  ok: screenshot inworld" || { echo "  FAILED: no inworld.png" >&2; fail=1; }
if ((fail)); then
  echo "e2e-native: FAILED (KEEP=1 keeps the logs)" >&2
  exit 1
fi
echo "e2e-native: OK"
