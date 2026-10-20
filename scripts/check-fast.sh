#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fast gate for every commit: only what the change can affect.
#
#   - cargo fmt --check for the changed crates
#   - clippy -D warnings for the changed crates (bhpc-run with `play`)
#   - coverage (scripts/coverage.sh, 100 % lines + functions) for the changed crates, which also runs their tests
#   - cargo test for the crates that depend on them, plus the changed crates' doctests
#   - app: vitest with its 100 % thresholds, only when app/ changed
#
# "Changed" = files that differ from the merge base with $BASE (default: dev) plus staged, unstaged and untracked
# files. A change outside crates/ that affects the build (Cargo.toml, Cargo.lock, .cargo/, rust-toolchain, third_party/,
# scripts/coverage.sh) selects every crate. scripts/check-full.sh stays the gate before merging into dev.
#
# Usage: scripts/check-fast.sh [--dry-run]     (env: BASE=<ref>, CRATES="bhpc-x bhpc-y" to override the selection)
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/lib-cache.sh
source scripts/lib-cache.sh

dry=0
[[ "${1:-}" == "--dry-run" ]] && dry=1
base="${BASE:-dev}"

changed_files() {
  local mb
  mb=$(git merge-base HEAD "$base" 2>/dev/null || echo HEAD)
  {
    git diff --name-only "$mb"
    git ls-files --others --exclude-standard
  } | sort -u
}

all_crates() { cargo metadata --format-version 1 --no-deps | jq -r '.packages[].name' | sort; }

# Workspace members that depend (directly or not) on any of "$@", excluding "$@" themselves.
dependents() {
  cargo metadata --format-version 1 --no-deps | jq -r --arg sel "$*" '
    ($sel | split(" ")) as $s
    | [.packages[] | {name, deps: [.dependencies[] | select(.path != null) | .name]}] as $p
    | def grow($set): ([$p[] | select(any(.deps[]; . as $d | $set | index($d))) | .name] + $set | unique) as $n
        | if ($n | length) == ($set | length) then $set else grow($n) end;
      grow($s) - $s | .[]'
}

files=$(changed_files)
app_changed=0
grep -q '^app/' <<<"$files" && app_changed=1

if [[ -n "${CRATES:-}" ]]; then
  crates=$CRATES
elif grep -qE '^(Cargo\.(toml|lock)|\.cargo/|rust-toolchain|third_party/|scripts/coverage\.sh|rustfmt\.toml)' <<<"$files"; then
  crates=$(all_crates | tr '\n' ' ')
else
  crates=$(grep -oE '^crates/[^/]+/' <<<"$files" | cut -d/ -f2 | sort -u | tr '\n' ' ' || true)
fi
crates=$(echo $crates)
deps=""
[[ -n "$crates" ]] && deps=$(echo $(dependents $crates))

echo "check-fast: base=$base"
echo "  changed crates: ${crates:-none}"
echo "  dependents:     ${deps:-none}"
echo "  app changed:    $([[ $app_changed == 1 ]] && echo yes || echo no)"
((dry)) && exit 0

t0=$SECONDS
step() { local s=$SECONDS; echo "== $*"; "$@"; echo "   ($((SECONDS - s)) s)"; }

if [[ -n "$crates" ]]; then
  pkgs=()
  feats=()
  for c in $crates; do
    pkgs+=(-p "$c")
    [[ "$c" == bhpc-run ]] && feats=(--features bhpc-run/play)
  done
  step cargo fmt "${pkgs[@]}" --check
  step cargo clippy "${pkgs[@]}" "${feats[@]}" --all-targets -- -D warnings
  # Runs the changed crates' unit and integration tests under instrumentation and enforces 100 %.
  COVERAGE_PACKAGES="$crates" step scripts/coverage.sh
  step cargo test "${pkgs[@]}" --doc
  if [[ -n "$deps" ]]; then
    dpkgs=()
    for c in $deps; do dpkgs+=(-p "$c"); done
    step cargo test "${dpkgs[@]}"
  fi
fi

if ((app_changed)); then
  (cd app && step bun install --frozen-lockfile && step bun run test)
fi

echo "check-fast: OK in $((SECONDS - t0)) s"
