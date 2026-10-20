// SPDX-License-Identifier: GPL-3.0-or-later
// M0 fixture: C++ exceptions unwinding through the ARM EHABI tables.
#include <cstdint>

struct Boom { int code; };
struct Guard {
  int *flag;
  explicit Guard(int *f) : flag(f) {}
  ~Guard() { ++*flag; }
};

static int g_cleanups = 0;

__attribute__((noinline)) static void thrower(int depth) {
  Guard g(&g_cleanups);
  if (depth == 0) throw Boom{42};
  thrower(depth - 1);
}

__attribute__((noinline)) static int catcher() {
  try {
    thrower(3);
  } catch (const Boom &b) {
    return b.code;
  }
  return -1;
}

__attribute__((noinline)) static int catch_all() {
  try {
    try {
      throw 7;
    } catch (...) {
      throw;  // rethrow
    }
  } catch (int v) {
    return v;
  }
  return -1;
}

extern "C" int bhpc_cpp_selftest(void) {
  if (catcher() != 42) return 1;
  if (g_cleanups != 4) return 2;
  if (catch_all() != 7) return 3;
  return 0;
}
