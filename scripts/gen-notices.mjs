#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Writes app/static/third-party-notices.txt: the licences shown under Settings > Updates & about > "Show all
// licences". It lists every Rust crate behind the launcher and the game runner and every npm package the UI ships,
// with its SPDX licence, and reproduces the font licences (OFL-1.1 requires the notice).
//
// Usage: node scripts/gen-notices.mjs        (needs cargo and `bun install` done in app/)
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'app', 'static', 'third-party-notices.txt');

function crates(manifest, extra = []) {
  const json = execFileSync('cargo', ['metadata', '--format-version', '1', '--locked', '--manifest-path', manifest, ...extra], {
    maxBuffer: 1 << 28
  });
  const meta = JSON.parse(json);
  const members = new Set(meta.workspace_members);
  return meta.packages.filter((p) => !members.has(p.id) && !p.source?.startsWith('path+')).map((p) => ({
    name: p.name,
    version: p.version,
    license: p.license ?? 'see crate',
    url: p.repository ?? p.homepage ?? ''
  }));
}

const all = new Map();
for (const c of [
  ...crates(join(root, 'app/src-tauri/Cargo.toml')),
  ...crates(join(root, 'Cargo.toml'), ['--filter-platform', 'x86_64-unknown-linux-gnu'])
])
  all.set(`${c.name}@${c.version}`, c);
const rows = [...all.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

const app = join(root, 'app');
const pkg = JSON.parse(readFileSync(join(app, 'package.json'), 'utf8'));
const npm = ['svelte', '@sveltejs/kit', ...Object.keys(pkg.dependencies)].map((name) => {
  const p = JSON.parse(readFileSync(join(app, 'node_modules', name, 'package.json'), 'utf8'));
  return { name, version: p.version, license: p.license };
});

const fonts = [
  '@fontsource-variable/bricolage-grotesque',
  '@fontsource/atkinson-hyperlegible-next',
  '@fontsource/jetbrains-mono',
  '@fontsource-variable/onest'
];
const fontTexts = fonts.map((name) => {
  const dir = join(app, 'node_modules', name);
  const file = ['LICENSE', 'LICENSE.md', 'LICENSE.txt'].map((f) => join(dir, f)).find(existsSync);
  return `--- ${name} ---\n${file ? readFileSync(file, 'utf8').trim() : 'SIL Open Font License 1.1, see https://openfontlicense.org'}`;
});

const text = `Blockheads PC
=============
Blockheads PC is free software, licensed under the GNU General Public License, version 3 or (at your option) any later version (GPL-3.0-or-later).
The full text is in the LICENSE file next to this program. It contains no code or assets of the game: you supply
your own copy of The Blockheads. It is an unofficial fan project and is not affiliated with the game's developers.

Bundled third-party software
============================
dynarmic (ARM JIT, vendored): 0BSD, with fmt (MIT), mcl (MIT), robin-map (MIT), xbyak (BSD-3-Clause), zydis (MIT),
zycore (MIT) and oaknut (MIT).
SDL2 (statically linked into the game runner): zlib licence, https://libsdl.org

Rust crates (${rows.length})
${rows.map((r) => `  ${r.name} ${r.version}  ${r.license}`).join('\n')}

npm packages shipped in the interface
${npm.map((r) => `  ${r.name} ${r.version}  ${r.license}`).join('\n')}

Font licences
=============
${fontTexts.join('\n\n')}
`;
writeFileSync(out, text);
console.log(`wrote ${out} (${rows.length} crates, ${npm.length} npm packages)`);
