#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Sets the release version in every file that carries it: `node scripts/set-version.mjs 0.4.0`.
// semantic-release runs it in its prepare step (.releaserc.json); the files it touches are the `assets` of
// @semantic-release/git there. Every pattern must match, so a moved version string fails the release instead of
// shipping a half-bumped tree.
import { readFileSync, writeFileSync } from 'node:fs';

const next = process.argv[2] ?? '';
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(next)) {
  console.error('usage: node scripts/set-version.mjs X.Y.Z[-pre.N]');
  process.exit(1);
}
const current = JSON.parse(readFileSync('app/package.json', 'utf8')).version;
const v = current.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const nl = '\\r?\\n';
// Local crates are the ones without a `source` line (registry and git crates always have one).
const lockLocal = () => new RegExp(`(name = "[^"]+"${nl}version = ")${v}("${nl}(?!source))`, 'g');

// Each pattern captures the text before and after the current version. `g` patterns replace every match.
const edits = [
  ['README.md', new RegExp(`(\\(v)${v}( pre-release\\))`)],
  ['Cargo.toml', new RegExp(`(\\[workspace\\.package\\]${nl}version = ")${v}(")`)],
  ['app/package.json', new RegExp(`("version": ")${v}(")`)],
  ['app/src-tauri/Cargo.toml', new RegExp(`(${nl}version = ")${v}(")`)],
  ['app/src-tauri/tauri.conf.json', new RegExp(`("version": ")${v}(")`)],
  ['Cargo.lock', lockLocal()],
  ['app/src-tauri/Cargo.lock', lockLocal()],
];

for (const [file, re] of edits) {
  const text = readFileSync(file, 'utf8');
  if (!re.test(text)) {
    console.error(`${file}: ${current} not found (${re})`);
    process.exit(1);
  }
  re.lastIndex = 0;
  writeFileSync(file, text.replace(re, (_, before, after) => before + next + after));
}
console.log(`version ${current} -> ${next}`);
