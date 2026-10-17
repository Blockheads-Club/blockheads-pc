#!/usr/bin/env bash
# Enforce 100% line and function coverage of bhpc-loader's own code.
# Excluded: the test-ELF/zip builder (src/testelf.rs). Needs cargo-llvm-cov.
# Region coverage is reported but not gated: it counts every `?` error-propagation edge on guest-memory reads.
set -euo pipefail
cd "$(dirname "$0")"
# Private target dir so concurrent coverage runs of other crates cannot mix their profiles in.
export CARGO_LLVM_COV_TARGET_DIR="${CARGO_LLVM_COV_TARGET_DIR:-$PWD/../../target/llvm-cov-loader}"
exec cargo llvm-cov -p bhpc-loader --features test-elf \
    --ignore-filename-regex '(testelf\.rs|/crates/bhpc-(mem|cpu|bionic|run|jni|gles|audio|app)/|/third_party/|/\.cargo/|/rustc/)' \
    --fail-under-lines 100 --fail-under-functions 100 \
    "$@"
