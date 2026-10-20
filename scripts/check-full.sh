#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Full gate before merging into dev (CI runs the same checks):
#
#   Rust   cargo fmt --check; clippy -D warnings on the whole workspace with bhpc-run's `play` feature;
#          scripts/coverage.sh, which runs every unit and integration test of the workspace (instrumented) and
#          enforces 100 % lines + functions; the doctests (not part of llvm-cov's run).
#   App    bun run check (svelte-check), vitest with its 100 % thresholds, Playwright e2e on a free port.
#          Runs in parallel with the Rust checks; its log is printed at the end.
#   Game   scripts/smoke-game.sh: one headless boot -> menu -> new world -> in-world run under a time limit
#          (skipped without the user's APK).
#
# Usage: scripts/check-full.sh [--no-smoke]
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/lib-cache.sh
source scripts/lib-cache.sh

smoke=1
[[ "${1:-}" == "--no-smoke" ]] && smoke=0

t0=$SECONDS
step() { local s=$SECONDS; echo "== $*"; "$@"; echo "   ($((SECONDS - s)) s)"; }

free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }

applog="$(mktemp "${TMPDIR:-/tmp}/bhpc-check-app.XXXXXX")"
(
  set -euo pipefail
  cd app
  s=$SECONDS
  bun install --frozen-lockfile
  bun run check
  bun run test
  CI=1 PW_PORT="$(free_port)" bun run test:e2e
  echo "app checks: OK ($((SECONDS - s)) s)"
) > "$applog" 2>&1 &
app_pid=$!
# Never leave the app checks running when the Rust part fails.
trap 'kill "$app_pid" 2> /dev/null || true; rm -f "$applog"' EXIT

step cargo fmt --all --check
step cargo clippy --workspace --all-targets --features bhpc-run/play -- -D warnings
step scripts/coverage.sh
step cargo test --workspace --doc

echo "== app checks (waiting)"
if ! wait "$app_pid"; then
  cat "$applog"
  echo "check-full: app checks FAILED" >&2
  exit 1
fi
tail -n 1 "$applog"

if ((smoke)); then
  step scripts/smoke-game.sh
fi

echo "check-full: OK in $((SECONDS - t0)) s"
