/* SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Test fixture of the PC runner's libbhmod host (crates/bhpc-run/src/tests_native.rs). Every call of the table is
 * made once and its result is stored in bhmod_test_results[], which the test reads from guest memory. The functions
 * it hooks belong to the fake game of the test. Own code; built by fixtures/native/build.sh.
 */
#include "bhmod.h"

BHMOD_EXPORT int bhmod_test_results[16];
BHMOD_EXPORT int bhmod_test_ctor;
__attribute__((constructor)) static void ctor(void) {
  __asm__ volatile("" ::: "memory"); /* keeps the compiler from folding the constructor into the data */
  bhmod_test_ctor = 42;
}

static const bhmod_api_t *api;
static int (*orig_add)(int, int);
static int (*orig_objc)(void *, void *, int, int);
static int (*orig_addr)(int, int);

static int my_add(int a, int b) { return orig_add(a, b) + 100; }
static int my_objc(void *self, void *cmd, int a, int b) { return orig_objc(self, cmd, a, b) * 2; }
static int my_addr(int a, int b) { return orig_addr(a, b) + 1000; }

static int starts_with_n(const char *s) { return s && s[0] == 'n'; }

BHMOD_EXPORT int bhmod_native_init(const bhmod_api_t *a) {
  api = a;
  int *r = bhmod_test_results;
  api->log(api, BHMOD_LOG_INFO, "testmod init");
  api->log(api, BHMOD_LOG_WARN, 0);
  r[0] = api->hook_symbol(api, 0, "test_add", (void *)my_add, (void **)&orig_add);
  r[1] = api->hook_objc(api, "Calc", "add:to:", 0, (void *)my_objc, (void **)&orig_objc);
  r[2] = api->hook_address(api, "libfake.so", 0x40, (void *)my_addr, (void **)&orig_addr);
  r[3] = api->hook_symbol(api, 0, "nope", (void *)my_add, 0);
  r[4] = starts_with_n(api->last_error(api));
  r[5] = api->hook_objc(api, 0, "x", 0, (void *)my_add, 0);
  r[6] = api->find_symbol(api, "libfake.so", "test_add") != 0;
  r[7] = api->module_base(api, "libfake.so") == (void *)0x20100000;
  r[8] = api->unhook(api, r[2]);
  r[9] = api->hook_import(api, 0, "nothing", (void *)my_add, 0);
  r[10] = api->hook_symbol(api, 0, 0, (void *)my_add, 0);
  r[11] = api->hook_address(api, 0, 0, (void *)my_add, 0);
  r[12] = api->hook_import(api, 0, 0, (void *)my_add, 0);
  r[13] = api->unhook(api, 9999);
  r[14] = api->find_symbol(api, 0, 0) == 0 && api->module_base(api, 0) == 0 && api->module_base(api, "libnone.so") == 0;
  return 0;
}

BHMOD_EXPORT void bhmod_native_shutdown(void) { bhmod_test_results[15] = 1; }
