/* SPDX-License-Identifier: GPL-3.0-or-later
 * Test fixture: init fails, so the runner must report it and undo what the mod hooked before failing. */
#include "bhmod.h"

static int (*orig_add)(int, int);
static int my_add(int a, int b) { return orig_add(a, b) + 1; }

BHMOD_EXPORT int bhmod_native_init(const bhmod_api_t *api) {
  api->hook_symbol(api, 0, "test_add", (void *)my_add, (void **)&orig_add);
  return 7;
}
