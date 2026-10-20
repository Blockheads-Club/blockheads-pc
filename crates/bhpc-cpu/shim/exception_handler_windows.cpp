// SPDX-License-Identifier: GPL-3.0-or-later
// Fastmem fault handling for dynarmic on Windows (x86_64 and arm64), replacing the vendored
// backend/exception_handler_windows.cpp (cmake/CMakeLists.txt swaps the source).
//
// Why: the vendored x86_64 handler is generated code registered as the SEH language handler of the whole code
// cache; it calls a C++ lambda without the 32-byte shadow space the Windows x64 ABI requires, so the lambda's
// prologue (MSVC spills its register arguments there) overwrites the handler's own return address and the process
// dies with an access violation on the first guest fault. On arm64 Windows dynarmic has no handler at all (the
// generic one, so no fastmem). This one is a vectored exception handler, the Windows counterpart of
// exception_handler_posix.cpp: an access violation whose pc lies in a registered code cache is turned into a call
// of dynarmic's fastmem callback, exactly as the POSIX SIGSEGV handler does.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <algorithm>
#include <functional>
#include <memory>
#include <mutex>
#include <vector>

#include <mcl/bit_cast.hpp>
#include <mcl/macro/architecture.hpp>
#include <mcl/stdint.hpp>

#include "dynarmic/backend/exception_handler.h"

#if defined(MCL_ARCHITECTURE_X86_64)
#    include "dynarmic/backend/x64/block_of_code.h"
#elif defined(MCL_ARCHITECTURE_ARM64)
#    include <oaknut/code_block.hpp>
#else
#    error "Unsupported architecture"
#endif

namespace Dynarmic::Backend {

namespace {

struct CodeBlockInfo {
    u64 code_begin, code_end;
    std::function<FakeCall(u64)> cb;
};

std::mutex blocks_mutex;
std::vector<CodeBlockInfo> blocks;
std::once_flag install_once;
bool installed = false;

LONG CALLBACK VectoredHandler(PEXCEPTION_POINTERS info) {
    if (info->ExceptionRecord->ExceptionCode != EXCEPTION_ACCESS_VIOLATION) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    CONTEXT* ctx = info->ContextRecord;
#if defined(MCL_ARCHITECTURE_X86_64)
    const u64 pc = ctx->Rip;
#else
    const u64 pc = ctx->Pc;
#endif
    std::lock_guard<std::mutex> guard(blocks_mutex);
    const auto it = std::find_if(blocks.begin(), blocks.end(),
                                 [&](const CodeBlockInfo& b) { return b.code_begin <= pc && pc < b.code_end && b.cb; });
    if (it == blocks.end()) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    const FakeCall fc = it->cb(pc);
#if defined(MCL_ARCHITECTURE_X86_64)
    ctx->Rsp -= sizeof(u64);
    *mcl::bit_cast<u64*>(ctx->Rsp) = fc.ret_rip;
    ctx->Rip = fc.call_rip;
#else
    ctx->Pc = fc.call_pc;
#endif
    return EXCEPTION_CONTINUE_EXECUTION;
}

void Install() {
    std::call_once(install_once, [] { installed = AddVectoredExceptionHandler(1, VectoredHandler) != nullptr; });
}

}  // namespace

struct ExceptionHandler::Impl final {
    Impl(u64 begin, u64 end)
            : code_begin(begin), code_end(end) {
        Install();
    }

    void SetCallback(std::function<FakeCall(u64)> cb) {
        std::lock_guard<std::mutex> guard(blocks_mutex);
        blocks.erase(std::remove_if(blocks.begin(), blocks.end(), [&](const CodeBlockInfo& b) { return b.code_begin == code_begin; }),
                     blocks.end());
        blocks.push_back(CodeBlockInfo{code_begin, code_end, std::move(cb)});
    }

    ~Impl() {
        std::lock_guard<std::mutex> guard(blocks_mutex);
        blocks.erase(std::remove_if(blocks.begin(), blocks.end(), [&](const CodeBlockInfo& b) { return b.code_begin == code_begin; }),
                     blocks.end());
    }

    u64 code_begin, code_end;
};

ExceptionHandler::ExceptionHandler() = default;
ExceptionHandler::~ExceptionHandler() = default;

#if defined(MCL_ARCHITECTURE_X86_64)
void ExceptionHandler::Register(X64::BlockOfCode& code) {
    const u64 code_begin = mcl::bit_cast<u64>(code.getCode());
    impl = std::make_unique<Impl>(code_begin, code_begin + code.GetTotalCodeSize());
}
#else
void ExceptionHandler::Register(oaknut::CodeBlock& mem, std::size_t size) {
    const u64 code_begin = mcl::bit_cast<u64>(mem.ptr());
    impl = std::make_unique<Impl>(code_begin, code_begin + size);
}
#endif

bool ExceptionHandler::SupportsFastmem() const noexcept {
    return static_cast<bool>(impl) && installed;
}

void ExceptionHandler::SetFastmemCallback(std::function<FakeCall(u64)> cb) {
    impl->SetCallback(std::move(cb));
}

}  // namespace Dynarmic::Backend
