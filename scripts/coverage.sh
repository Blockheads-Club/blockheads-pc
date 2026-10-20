#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Coverage gate: 100% of lines and functions of our own Rust code (see ../AGENTS.md).
#
# Excluded from measurement, and nothing else:
#   - third_party/            vendored dynarmic (verbatim copy, see third_party/dynarmic/VENDORED.md)
#   - the C++ shim            crates/bhpc-cpu/shim (not Rust, so llvm-cov never sees it)
#   - test code               tests.rs / tests_*.rs, crates/*/tests/ and the test-ELF builder
#                             crates/bhpc-loader/src/testelf.rs
#
# Usage: scripts/coverage.sh [--html | --lcov FILE] [extra cargo-llvm-cov args]
# Set COVERAGE_PACKAGES="bhpc-cpu bhpc-mem" to gate only some crates (default: the whole workspace).
# Needs: cargo-llvm-cov and the llvm-tools-preview rustup component.
set -euo pipefail
cd "$(dirname "$0")/.."

EXCLUDE='(/tests(_[a-z]+)?\.rs$|/crates/[^/]+/tests/|/testelf\.rs$|/third_party/)'

mode=(--summary-only)
if [[ "${1:-}" == "--html" ]]; then
  mode=(--html); shift
elif [[ "${1:-}" == "--lcov" ]]; then
  mode=(--lcov --output-path "${2:?--lcov needs a file}"); shift 2
fi

scope=(--workspace)
if [[ -n "${COVERAGE_PACKAGES:-}" ]]; then
  scope=()
  for p in $COVERAGE_PACKAGES; do scope+=(-p "$p"); done
fi

exec cargo llvm-cov "${scope[@]}" \
  --ignore-filename-regex "$EXCLUDE" \
  --fail-under-lines 100 --fail-under-functions 100 \
  "${mode[@]}" "$@"
