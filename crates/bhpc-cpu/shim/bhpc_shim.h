// SPDX-License-Identifier: GPL-3.0-only
// C ABI over dynarmic's A32 JIT (see bhpc_shim.cpp). Guest memory is reached through a fastmem
// window (host pointer + 32-bit guest address) with callbacks as the fallback path.
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct bhpc_jit bhpc_jit;

typedef struct bhpc_callbacks {
    void* user;
    uint8_t (*read8)(void* user, uint32_t addr);
    uint16_t (*read16)(void* user, uint32_t addr);
    uint32_t (*read32)(void* user, uint32_t addr);
    uint64_t (*read64)(void* user, uint32_t addr);
    void (*write8)(void* user, uint32_t addr, uint8_t v);
    void (*write16)(void* user, uint32_t addr, uint16_t v);
    void (*write32)(void* user, uint32_t addr, uint32_t v);
    void (*write64)(void* user, uint32_t addr, uint64_t v);
    // Exclusive stores (strex): store `value` iff the memory still equals `expected`; return 1 on success.
    uint8_t (*write_excl8)(void* user, uint32_t addr, uint8_t value, uint8_t expected);
    uint8_t (*write_excl16)(void* user, uint32_t addr, uint16_t value, uint16_t expected);
    uint8_t (*write_excl32)(void* user, uint32_t addr, uint32_t value, uint32_t expected);
    uint8_t (*write_excl64)(void* user, uint32_t addr, uint64_t value, uint64_t expected);
    // Guest executed `svc #swi`. The callback may read/write registers and call bhpc_jit_halt.
    void (*svc)(void* user, uint32_t swi);
    // Undefined instruction, breakpoint, etc. `kind` is dynarmic's A32::Exception value.
    void (*exception)(void* user, uint32_t pc, uint32_t kind);
    // Instruction dynarmic cannot translate and wants interpreted (we cannot).
    void (*interpreter_fallback)(void* user, uint32_t pc, size_t num_instructions);
} bhpc_callbacks;

// `fastmem_base` may be NULL to force the callback path for every access.
bhpc_jit* bhpc_jit_new(const bhpc_callbacks* cb, uint8_t* fastmem_base, size_t code_cache_bytes, int cycle_counting);
// Only meaningful when created with cycle_counting: run stops when the budget is used up.
void bhpc_jit_set_ticks(bhpc_jit* j, uint64_t ticks);
uint64_t bhpc_jit_ticks(bhpc_jit* j);
void bhpc_jit_free(bhpc_jit* j);
// Returns dynarmic's HaltReason bitmask.
// `escape` is ignored; passing the owning object's address makes the compiler assume the JIT callbacks
// (which re-enter Rust through a raw pointer) may have modified it.
uint32_t bhpc_jit_run(bhpc_jit* j, void* escape);
// Executes exactly one instruction; returns the HaltReason bitmask.
uint32_t bhpc_jit_step(bhpc_jit* j, void* escape);
void bhpc_jit_halt(bhpc_jit* j);
void bhpc_jit_clear_halt(bhpc_jit* j);
void bhpc_jit_invalidate(bhpc_jit* j, uint32_t start, size_t len);
void bhpc_jit_clear_cache(bhpc_jit* j);
uint32_t* bhpc_jit_regs(bhpc_jit* j);      // 16 entries (r0..r15)
uint32_t* bhpc_jit_ext_regs(bhpc_jit* j);  // 64 entries (s0..s63 / d0..d31 as pairs)
uint32_t bhpc_jit_cpsr(bhpc_jit* j);
void bhpc_jit_set_cpsr(bhpc_jit* j, uint32_t v);
uint32_t bhpc_jit_fpscr(bhpc_jit* j);
void bhpc_jit_set_fpscr(bhpc_jit* j, uint32_t v);

#ifdef __cplusplus
}
#endif
