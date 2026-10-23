#!/usr/bin/env bash
# Builds the native test fixtures (libtestmod.so, libfailmod.so, libnoinit.so, libcrashmod.so) with the Android NDK. The results are
# committed (a few KiB of our own code) so the tests need no NDK. Header: src/bhmod.h is a copy of mod-sdk's
# crates/bhmod-native/include/bhmod.h.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}"
bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
for m in testmod failmod noinit crashmod; do
  "$bin/armv7a-linux-androideabi24-clang" -marm -O2 -fPIC -shared -nostdlib -ffreestanding -fno-stack-protector \
    -fvisibility=hidden -fno-asynchronous-unwind-tables -Wall -Werror -Wl,--build-id=none -Wl,--no-undefined \
    -Wl,-z,max-page-size=4096 -Wl,-soname,lib$m.so -I "$here/src" "$here/src/$m.c" -o "$here/lib$m.so"
  "$bin/llvm-strip" --strip-unneeded "$here/lib$m.so"
done
ls -l "$here"/*.so
