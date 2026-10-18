#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-only
// Tauri's beforeDevCommand / beforeBuildCommand: refresh the game runner sidecar (app/src-tauri/binaries/bhpc-run-<triple>)
// from the current sources, so a launcher build can never bundle a stale runner. `scripts/build-sidecar.sh` is
// incremental (cargo): fast when nothing changed. BHPC_SKIP_SIDECAR=1 skips it (CI that built the sidecar for another
// target triple in an earlier step and passes it through).
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

if (process.env.BHPC_SKIP_SIDECAR === '1') {
  console.log('ensure-sidecar: skipped (BHPC_SKIP_SIDECAR=1)');
  process.exit(0);
}
const script = join(dirname(fileURLToPath(import.meta.url)), 'build-sidecar.sh');
const r = spawnSync('bash', [script, ...process.argv.slice(2)], { stdio: 'inherit' });
if (r.error) console.error(`ensure-sidecar: cannot run bash: ${r.error.message}`);
process.exit(r.status ?? 1);
