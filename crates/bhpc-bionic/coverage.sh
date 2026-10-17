#!/bin/sh
# Coverage gate for bhpc-bionic (AGENTS.md: 100% of our own code). Fails below 100% lines or functions.
# Excludes only vendored code, the C++ shim and the test-ELF builder; the test scaffolding is
# test-only code and is excluded as well.
set -eu
cd "$(dirname "$0")/../.."
exec cargo llvm-cov -p bhpc-bionic \
    --ignore-filename-regex 'third_party|/shim/|testelf|testenv' \
    --fail-under-lines 100 --fail-under-functions 100 "$@"
