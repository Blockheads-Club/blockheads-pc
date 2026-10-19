// SPDX-License-Identifier: GPL-3.0-only
// Thin C ABI wrapper around dynarmic's A32 JIT. Excluded from Rust coverage (C++ glue).
#include "bhpc_shim.h"

#include <memory>

#include <dynarmic/interface/A32/a32.h>
#include <dynarmic/interface/A32/config.h>
#include <dynarmic/interface/exclusive_monitor.h>
#include <dynarmic/interface/optimization_flags.h>

#include "dynarmic/frontend/A32/a32_ir_emitter.h"
#include "dynarmic/ir/basic_block.h"

namespace {

struct Callbacks final : Dynarmic::A32::UserCallbacks {
    bhpc_callbacks cb;
    std::uint64_t ticks = UINT64_MAX / 2;

    std::uint8_t MemoryRead8(std::uint32_t a) override { return cb.read8(cb.user, a); }
    std::uint16_t MemoryRead16(std::uint32_t a) override { return cb.read16(cb.user, a); }
    std::uint32_t MemoryRead32(std::uint32_t a) override { return cb.read32(cb.user, a); }
    std::uint64_t MemoryRead64(std::uint32_t a) override { return cb.read64(cb.user, a); }
    void MemoryWrite8(std::uint32_t a, std::uint8_t v) override { cb.write8(cb.user, a, v); }
    void MemoryWrite16(std::uint32_t a, std::uint16_t v) override { cb.write16(cb.user, a, v); }
    void MemoryWrite32(std::uint32_t a, std::uint32_t v) override { cb.write32(cb.user, a, v); }
    void MemoryWrite64(std::uint32_t a, std::uint64_t v) override { cb.write64(cb.user, a, v); }

    bool MemoryWriteExclusive8(std::uint32_t a, std::uint8_t v, std::uint8_t e) override { return cb.write_excl8(cb.user, a, v, e) != 0; }
    bool MemoryWriteExclusive16(std::uint32_t a, std::uint16_t v, std::uint16_t e) override { return cb.write_excl16(cb.user, a, v, e) != 0; }
    bool MemoryWriteExclusive32(std::uint32_t a, std::uint32_t v, std::uint32_t e) override { return cb.write_excl32(cb.user, a, v, e) != 0; }
    bool MemoryWriteExclusive64(std::uint32_t a, std::uint64_t v, std::uint64_t e) override { return cb.write_excl64(cb.user, a, v, e) != 0; }

    void InterpreterFallback(std::uint32_t pc, std::size_t n) override { cb.interpreter_fallback(cb.user, pc, n); }
    void CallSVC(std::uint32_t swi) override { cb.svc(cb.user, swi); }
    void ExceptionRaised(std::uint32_t pc, Dynarmic::A32::Exception e) override {
        cb.exception(cb.user, pc, static_cast<std::uint32_t>(e));
    }
    // Two guards on the instruction about to be translated.
    //  * An all-zero word is `andeq r0, r0, r0`, which no compiler emits: executing one means a jump into
    //    zero-filled memory (bss, an unrelocated function pointer). Treat it as undefined so the stop is
    //    reported where it happens. Running on would also be pathological: a run of conditional
    //    instructions becomes one huge block, and dynarmic's conditional bookkeeping rescans the whole
    //    block per instruction (quadratic time).
    //  * Cap the length of an unconditional block for the same quadratic-time reason (huge straight-line
    //    static initialisers).
    bool PreCodeReadHook(bool is_thumb, std::uint32_t pc, Dynarmic::A32::IREmitter& ir) override {
        constexpr std::uint64_t kMaxBlockInstructions = 256;
        if (!is_thumb && MemoryRead32(pc) == 0) {
            ir.UpdateUpperLocationDescriptor();
            ir.BranchWritePC(ir.Imm32(pc + 4));
            ir.ExceptionRaised(Dynarmic::A32::Exception::UndefinedInstruction);
            ir.SetTerm(Dynarmic::IR::Term::CheckHalt{Dynarmic::IR::Term::ReturnToDispatch{}});
            return false;
        }
        if (ir.block.CycleCount() >= kMaxBlockInstructions && ir.block.GetCondition() == Dynarmic::IR::Cond::AL) {
            ir.SetTerm(Dynarmic::IR::Term::LinkBlockFast{ir.current_location});
            return false;
        }
        return true;
    }
    void AddTicks(std::uint64_t t) override { ticks = t > ticks ? 0 : ticks - t; }
    std::uint64_t GetTicksRemaining() override { return ticks; }
};

}  // namespace

struct bhpc_jit {
    Callbacks callbacks;
    std::unique_ptr<Dynarmic::A32::Jit> jit;
};

struct bhpc_monitor {
    explicit bhpc_monitor(size_t n) : monitor(n) {}
    Dynarmic::ExclusiveMonitor monitor;
};

extern "C" {

bhpc_monitor* bhpc_monitor_new(size_t processors) { return new bhpc_monitor(processors); }
void bhpc_monitor_free(bhpc_monitor* m) { delete m; }

bhpc_jit* bhpc_jit_new(const bhpc_callbacks* cb, uint8_t* fastmem_base, size_t code_cache_bytes, int cycle_counting,
                       bhpc_monitor* monitor, size_t processor_id, uint32_t flags) {
    auto* j = new bhpc_jit();
    j->callbacks.cb = *cb;
    Dynarmic::A32::UserConfig conf;
    conf.callbacks = &j->callbacks;
    conf.global_monitor = &monitor->monitor;
    conf.processor_id = processor_id;
    conf.arch_version = Dynarmic::A32::ArchVersion::v7;
    conf.enable_cycle_counting = cycle_counting != 0;
    conf.code_cache_size = code_cache_bytes;
    if (fastmem_base) {
        conf.fastmem_pointer = reinterpret_cast<uintptr_t>(fastmem_base);
        conf.recompile_on_fastmem_failure = true;
        // ldrex/strex inline against the host mapping (lock cmpxchg under the global monitor's lock) instead of a
        // call into Rust per access. A fault (unmapped address) recompiles that access with the callbacks, which
        // report it as before.
        if (flags & BHPC_JIT_FASTMEM_EXCLUSIVE) {
            conf.fastmem_exclusive_access = true;
            conf.recompile_on_exclusive_fastmem_failure = true;
        }
    }
    if (flags & BHPC_JIT_FAST_FP) {
        // NaN payloads need not match ARM's propagation rules, and NEON may use the guest FPSCR instead of the
        // "standard FPSCR value": saves the NaN fix-up sequences and MXCSR switches around float operations.
        conf.unsafe_optimizations = true;
        conf.optimizations = Dynarmic::all_safe_optimizations | Dynarmic::OptimizationFlag::Unsafe_InaccurateNaN |
                             Dynarmic::OptimizationFlag::Unsafe_IgnoreStandardFPCRValue;
    }
    j->jit = std::make_unique<Dynarmic::A32::Jit>(conf);
    return j;
}

void bhpc_jit_free(bhpc_jit* j) { delete j; }
void bhpc_jit_set_ticks(bhpc_jit* j, uint64_t t) { j->callbacks.ticks = t; }
uint64_t bhpc_jit_ticks(bhpc_jit* j) { return j->callbacks.ticks; }
uint32_t bhpc_jit_run(bhpc_jit* j, void*) { return static_cast<uint32_t>(j->jit->Run()); }
uint32_t bhpc_jit_step(bhpc_jit* j, void*) { return static_cast<uint32_t>(j->jit->Step()); }
void bhpc_jit_halt(bhpc_jit* j) { j->jit->HaltExecution(); }
void bhpc_jit_clear_halt(bhpc_jit* j) { j->jit->ClearHalt(); }
void bhpc_jit_invalidate(bhpc_jit* j, uint32_t start, size_t len) { j->jit->InvalidateCacheRange(start, len); }
void bhpc_jit_clear_cache(bhpc_jit* j) { j->jit->ClearCache(); }
uint32_t* bhpc_jit_regs(bhpc_jit* j) { return j->jit->Regs().data(); }
uint32_t* bhpc_jit_ext_regs(bhpc_jit* j) { return j->jit->ExtRegs().data(); }
uint32_t bhpc_jit_cpsr(bhpc_jit* j) { return j->jit->Cpsr(); }
void bhpc_jit_set_cpsr(bhpc_jit* j, uint32_t v) { j->jit->SetCpsr(v); }
uint32_t bhpc_jit_fpscr(bhpc_jit* j) { return j->jit->Fpscr(); }
void bhpc_jit_set_fpscr(bhpc_jit* j, uint32_t v) { j->jit->SetFpscr(v); }

}
