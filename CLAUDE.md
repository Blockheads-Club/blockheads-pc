# CLAUDE.md

blockheads-pc: run the user's own Blockheads Android build (ARM32 `.so` libs) on PC through a Rust loader,
an embedded ARM JIT (dynarmic) and host re-implementations of Android system libraries. GPL-3.0-only.
Architecture and milestones: `docs/feasibility/PLAN.md` (read first), `docs/feasibility/C-execution-strategy.md`.
Umbrella rules: `../AGENTS.md`.

## Rules
- Never commit or ship game files: no APKs, extracted `.so`, assets, decompiled code. Test APKs live in `~/Downloads`
  and are extracted only into the session scratchpad, never into the repo. Only our own fixtures are committed.
- Rust via rustup stable, Conventional Commits, branch `dev`. Do not push without being asked.
- 100% coverage for our own Rust crates (`cargo llvm-cov`); vendored `third_party/` and the C++ shim are excluded.
  Report real numbers and list what is not covered.
- `third_party/dynarmic` is a verbatim pinned copy (see `third_party/dynarmic/VENDORED.md`); do not edit it, patch in
  the shim instead.

## Gotchas learned in M0
- dynarmic forbids nested `Run()` on one `Jit`; host functions that must call guest code use `Cpu::tail_call`
  (thunk stubs return through `r12`).
- The JIT calls back into Rust through a raw pointer: always pass the owner to the FFI `run`/`step` (`escape`
  argument) so rustc does not cache fields across the call (this once made stops invisible in release builds).
- `strex` needs the `MemoryWriteExclusive*` callbacks; without them every store-exclusive fails and ARM spinlock
  loops (libc++abi refcounts) hang forever.
- Debug aids: `--trace-thunks`, `--watchdog CYCLES` (reports pc on expiry), `--trace-pc FILE` (single-step trace).
