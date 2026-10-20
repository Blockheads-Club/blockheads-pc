# SPDX-License-Identifier: GPL-3.0-or-later
# Sourced by the check scripts: build caches shared by every checkout and agent worktree.
#
#   - dynarmic: crates/bhpc-cpu/build.rs reuses prebuilt archives from ~/.cache/bhpc/dynarmic-<hash> (always on;
#     BHPC_DYNARMIC_CACHE=0 turns it off, BHPC_DYNARMIC_CACHE=DIR moves it).
#   - Rust: when sccache is installed (`cargo install sccache --locked --no-default-features`) it wraps rustc, so
#     crates.io dependencies are compiled once per machine instead of once per worktree. Workspace crates stay
#     incremental (sccache cannot cache those) and are rebuilt per target dir. BHPC_SCCACHE=0 opts out.
# Each worktree keeps its own target/: a shared CARGO_TARGET_DIR would serialise parallel agents on cargo's lock.
bhpc_sccache=$(command -v sccache || true)
[[ -z "$bhpc_sccache" && -x "${CARGO_HOME:-$HOME/.cargo}/bin/sccache" ]] && bhpc_sccache="${CARGO_HOME:-$HOME/.cargo}/bin/sccache"
if [[ "${BHPC_SCCACHE:-1}" != 0 && -z "${RUSTC_WRAPPER:-}" && -n "$bhpc_sccache" ]]; then
  export RUSTC_WRAPPER="$bhpc_sccache"
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-30G}"
fi
