#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Builds the game runner (`bhpc-run`, play build with a statically linked SDL2) and puts it where the launcher's
# Tauri bundle expects its sidecar: app/src-tauri/binaries/bhpc-run-<target triple>[.exe].
#
# Usage: scripts/build-sidecar.sh [TARGET_TRIPLE]   (default: the host triple)
# Needs: cmake and a C/C++ toolchain (SDL2 and dynarmic are built from source), plus the SDL build deps of the OS
# (Linux: libx11-dev libxext-dev libwayland-dev libxkbcommon-dev libasound2-dev libpulse-dev).
set -euo pipefail
cd "$(dirname "$0")/.."

triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
case "$triple" in *windows*) ext=".exe" ;; esac

# SDL2 2.28 declares an old cmake_minimum_required, which CMake 4 refuses without this.
export CMAKE_POLICY_VERSION_MINIMUM=3.5
# ...and its PipeWire backend does not compile against recent PipeWire headers with GCC 14+ (a pointer type warning
# that became an error). Keep it a warning.
export CFLAGS="${CFLAGS:-} -Wno-error=incompatible-pointer-types"

# A separate target dir: the static-SDL feature set must not thrash the everyday build in target/.
# Release builds ship without debug info (the workspace profile keeps it for profiling).
CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=symbols \
cargo build --release --locked -p bhpc-run --features "play bhpc-app/sdl-bundled" \
  --target "$triple" --target-dir target/sidecar

mkdir -p app/src-tauri/binaries
cp "target/sidecar/$triple/release/bhpc-run$ext" "app/src-tauri/binaries/bhpc-run-$triple$ext"
echo "sidecar: app/src-tauri/binaries/bhpc-run-$triple$ext"
