#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Fails when a Linux bundle (deb, rpm, AppImage) contains anything of the game: an APK, its libraries
# (libApplication.so, libv.so, ...) or an assets/ folder. Also lists what the bundles hold.
# Usage: scripts/check-bundle.sh <bundle dir>   (the directory `tauri build` writes: deb/, rpm/, appimage/)
set -euo pipefail
dir="${1:?bundle directory}"
bad='(\.apk$|libApplication|/libv\.so|/assets/|\.obb$)'
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
status=0
scan() { # name, file listing on stdin
  if grep -E "$bad" > "$work/hits"; then
    echo "FAIL $1 contains game files:"; cat "$work/hits"; status=1
  else
    echo "ok   $1"
  fi
}
for f in "$dir"/deb/*.deb; do
  [ -e "$f" ] || continue
  (cd "$work" && ar x "$f" && tar -tf data.tar.* ) | scan "$f"
done
for f in "$dir"/rpm/*.rpm; do
  [ -e "$f" ] || continue
  rpm -qlp "$f" | scan "$f"
done
for f in "$dir"/appimage/*.AppImage; do
  [ -e "$f" ] || continue
  rm -rf "$work/ai" && mkdir "$work/ai" && cp "$f" "$work/ai/app.AppImage" && chmod +x "$work/ai/app.AppImage"
  (cd "$work/ai" && ./app.AppImage --appimage-extract >/dev/null && find squashfs-root -type f -o -type l) | scan "$f"
done
exit $status
