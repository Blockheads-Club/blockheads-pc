/* SPDX-License-Identifier: GPL-3.0-or-later
 * Test fixture: init touches unmapped memory, so the guest stops inside bhmod_native_init. */
#include "bhmod.h"

BHMOD_EXPORT int bhmod_native_init(const bhmod_api_t *api) {
  (void)api;
  *(volatile int *)16 = 1;
  return 0;
}
