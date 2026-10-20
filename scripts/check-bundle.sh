#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Checks every bundle `tauri build` made: it must contain the game runner sidecar (bhpc-run) and nothing of the game
# (an APK, its libraries libApplication.so / libv.so, an assets/ folder). Lists what each bundle holds.
#   Linux: deb, rpm, AppImage   Windows: NSIS installer (7-Zip lists it), MSI (administrative install)
#   macOS: the .app and the dmg (mounted read-only)
# Usage: scripts/check-bundle.sh <bundle dir>   (the directory `tauri build` writes: deb/, rpm/, appimage/, nsis/, ...)
set -euo pipefail
dir="$(cd "${1:?bundle directory}" && pwd)"
bad='(\.apk$|libApplication|/libv\.so|[/\\]assets[/\\]|\.obb$)'
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
status=0
found=0
scan() { # name, file listing in "$work/list" (not a pipe: the counters must stay in this shell)
  found=$((found + 1))
  if grep -E "$bad" "$work/list" > "$work/hits"; then
    echo "FAIL $1 contains game files:"; cat "$work/hits"; status=1
  elif ! grep -Eq 'bhpc-run(\.exe)?$' "$work/list"; then
    echo "FAIL $1 has no game runner (bhpc-run):"; cat "$work/list"; status=1
  else
    echo "ok   $1 ($(wc -l < "$work/list") files)"
  fi
}
for f in "$dir"/deb/*.deb; do
  [ -e "$f" ] || continue
  (cd "$work" && ar x "$f" && tar -tf data.tar.* ) > "$work/list"
  scan "$f"
done
for f in "$dir"/rpm/*.rpm; do
  [ -e "$f" ] || continue
  rpm --dbpath "$work" -qlp "$f" > "$work/list"
  scan "$f"
done
for f in "$dir"/appimage/*.AppImage; do
  [ -e "$f" ] || continue
  rm -rf "$work/ai" && mkdir "$work/ai" && cp "$f" "$work/ai/app.AppImage" && chmod +x "$work/ai/app.AppImage"
  (cd "$work/ai" && ./app.AppImage --appimage-extract >/dev/null && find squashfs-root -type f -o -type l) > "$work/list"
  scan "$f"
done
for f in "$dir"/nsis/*.exe; do
  [ -e "$f" ] || continue
  7z l -ba -slt "$f" | sed -n 's/^Path = //p' > "$work/list"
  scan "$f"
done
for f in "$dir"/msi/*.msi; do
  [ -e "$f" ] || continue
  out="$work/msi-$found"
  msiexec //a "$(cygpath -w "$f")" //qn TARGETDIR="$(cygpath -w "$out")"
  find "$out" -type f > "$work/list"
  scan "$f"
done
for f in "$dir"/macos/*.app; do
  [ -e "$f" ] || continue
  find "$f" -type f -o -type l > "$work/list"
  scan "$f"
done
for f in "$dir"/dmg/*.dmg; do
  [ -e "$f" ] || continue
  mnt="$work/dmg-$found"
  mkdir -p "$mnt"
  # The dmg carries the GPL as a license agreement: attach asks to accept it on stdin.
  { yes || true; } | hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$f" >/dev/null
  find "$mnt" -type f -o -type l > "$work/list"
  scan "$f"
  hdiutil detach "$mnt" >/dev/null
done
if [ "$found" = 0 ]; then
  echo "FAIL no bundles in $dir"; status=1
fi
exit $status
