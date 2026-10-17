#!/usr/bin/env bash
# Builds fixtures/hello.so (armeabi-v7a, softfp, NEON, static libc++) with the Android NDK.
# SPDX-License-Identifier: GPL-3.0-only
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Android/Sdk/ndk/27.2.12479018}"
bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
cc="$bin/armv7a-linux-androideabi21-clang"
cxx="$bin/armv7a-linux-androideabi21-clang++"
flags="-O2 -fPIC -mfloat-abi=softfp -mfpu=neon -g0"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
$cc  $flags -c "$here/hello.c"   -o "$tmp/hello_c.o"
$cxx $flags -c "$here/hello.cpp" -o "$tmp/hello_cpp.o"
$cxx -shared -static-libstdc++ -Wl,--exclude-libs,ALL -Wl,-z,max-page-size=4096 \
  -o "$here/hello.so" "$tmp/hello_c.o" "$tmp/hello_cpp.o"
$bin/llvm-strip --strip-unneeded "$here/hello.so"
ls -l "$here/hello.so"
