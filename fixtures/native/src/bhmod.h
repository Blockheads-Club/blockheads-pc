/* SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * libbhmod: the C ABI of native Blockheads Club mods (client side, armeabi-v7a).
 *
 * A native mod is a shared library at client/native/armeabi-v7a/<name>.so inside the .bhmod package. It needs the
 * `native` permission, is listed in the public index with the same sha256 and is loaded only after the player has
 * confirmed the red warning. The loader dlopen()s it (Android: in the game process; PC: in the emulated guest) and
 * calls bhmod_native_init() once with the table below. There is nothing to link against: everything the mod needs
 * from the host is in the table.
 *
 * Hooks are named, not addressed: the host installs them. Replacement functions run on whatever game thread calls
 * the hooked code and must be short, thread-safe and must never throw. Build for ARM mode (-marm), without a C
 * runtime if possible (-nostdlib); the PC guest provides bionic's libc, liblog and libm only.
 *
 * Install hooks from bhmod_native_init(), before the game has used the code: patching a function while another
 * thread executes its first instructions is not atomic.
 */
#ifndef BHMOD_H
#define BHMOD_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define BHMOD_ABI_VERSION 1

/* The two entry points must stay visible when the library is built with -fvisibility=hidden. */
#define BHMOD_EXPORT __attribute__((visibility("default")))

/* Log levels of bhmod_api_t.log. */
#define BHMOD_LOG_TRACE 0
#define BHMOD_LOG_DEBUG 1
#define BHMOD_LOG_INFO 2
#define BHMOD_LOG_WARN 3
#define BHMOD_LOG_ERROR 4

/* Results of the hook_* functions: a handle (> 0) on success, one of these otherwise. bhmod_api_t.last_error()
 * describes the failure. */
#define BHMOD_E_ARG (-1)       /* a null or empty argument */
#define BHMOD_E_NOT_FOUND (-2) /* class, method, symbol, module or import does not exist (yet) */
#define BHMOD_E_THUMB (-3)     /* the target is Thumb code or not 4-byte aligned */
#define BHMOD_E_RELOC (-4)     /* the first two instructions cannot be moved into a trampoline */
#define BHMOD_E_EXISTS (-5)    /* the function is already hooked (one inline hook per function) */
#define BHMOD_E_MEMORY (-6)    /* game memory could not be read or patched */
#define BHMOD_E_HANDLE (-7)    /* unknown handle */

typedef struct bhmod_api bhmod_api_t;

struct bhmod_api {
  uint32_t abi;           /* BHMOD_ABI_VERSION */
  uint32_t size;          /* sizeof(bhmod_api_t) of the host: fields beyond it do not exist */
  const char *mod_id;     /* id of this mod (mod.toml) */
  const char *data_dir;   /* a private writable folder of the mod; may be NULL */

  /* Writes a line to the game log (logcat on Android, the runner's log on PC). */
  void (*log)(const bhmod_api_t *api, int level, const char *message);

  /* Replaces the Objective-C method -[cls selector] (+[cls selector] when class_method). *original receives the
   * previous implementation: call it as `ret (*)(id self, SEL _cmd, ...)`. */
  int (*hook_objc)(const bhmod_api_t *api, const char *cls, const char *selector, int class_method,
                   void *replacement, void **original);

  /* Inline-hooks the exported function `symbol` (of `module`, a file name such as "libApplication.so", or of any
   * loaded module when module is NULL). ARM-mode functions of at least two instructions only. *original is a
   * trampoline that runs the function's first instructions and continues in the rest of the original. */
  int (*hook_symbol)(const bhmod_api_t *api, const char *module, const char *symbol, void *replacement,
                     void **original);

  /* Inline-hooks the ARM-mode code at `offset` bytes from the load address of `module` (a virtual address of the
   * library file, as shown by a disassembler). Same rules as hook_symbol. */
  int (*hook_address)(const bhmod_api_t *api, const char *module, uint32_t offset, void *replacement,
                      void **original);

  /* Redirects the import `symbol` (PLT/GOT slot) of `module`, or of every loaded module when module is NULL, to
   * `replacement`. Other modules that are not patched, and the library's own definition, keep the original.
   * *original is the address the import resolved to. */
  int (*hook_import)(const bhmod_api_t *api, const char *module, const char *symbol, void *replacement,
                     void **original);

  /* Undoes a hook. Hooks of a mod are undone automatically when the mod is disabled. Returns 0 or a BHMOD_E_*. */
  int (*unhook)(const bhmod_api_t *api, int handle);

  /* The address of an exported symbol (dlsym), or NULL. */
  void *(*find_symbol)(const bhmod_api_t *api, const char *module, const char *symbol);

  /* The load address of a module, or NULL. */
  void *(*module_base)(const bhmod_api_t *api, const char *module);

  /* A sentence about the last failed call of this mod. Valid until the next call. */
  const char *(*last_error)(const bhmod_api_t *api);
};

/* Required: install hooks and return 0. A non-zero result disables the mod and undoes its hooks. */
BHMOD_EXPORT int bhmod_native_init(const bhmod_api_t *api);
/* Optional: called when the mod is disabled, before its hooks are undone. */
BHMOD_EXPORT void bhmod_native_shutdown(void);

#ifdef __cplusplus
}
#endif

#endif /* BHMOD_H */
