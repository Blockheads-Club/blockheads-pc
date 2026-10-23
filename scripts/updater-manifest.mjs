#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Writes the Tauri updater feed latest.json into a folder of release assets (launcher-bundles.yml, release job):
//   node scripts/updater-manifest.mjs <dir> <version> <tag>
// Every `<asset>.sig` in <dir> is the minisign signature of <asset>. The feed also carries each file's `size` in bytes
// (the launcher shows it before the user clicks install). Throws when a platform has no signed file, so a release
// never ships a partial feed.
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPO = 'Blockheads-Club/blockheads-pc';

/** Updater platform key -> end of the release file name that platform installs (names as GitHub stores them). */
export const PLATFORMS = {
  'windows-x86_64': '_x64-setup.exe',
  'windows-aarch64': '_arm64-setup.exe',
  'linux-x86_64': '_amd64.AppImage',
  'linux-aarch64': '_aarch64.AppImage',
  'darwin-x86_64': '_x64.app.tar.gz',
  'darwin-aarch64': '_aarch64.app.tar.gz'
};

/** files: asset name -> { sig: signature file content, size: bytes }. */
export function buildManifest({ version, tag, files, pubDate }) {
  const base = `https://github.com/${REPO}/releases/download/${tag}/`;
  const platforms = {};
  for (const [key, suffix] of Object.entries(PLATFORMS)) {
    const name = Object.keys(files).find((n) => n.endsWith(suffix));
    if (!name) throw new Error(`no signed updater file for ${key} (*${suffix})`);
    platforms[key] = { signature: files[name].sig.trim(), url: base + encodeURIComponent(name), size: files[name].size };
  }
  return { version, notes: `https://github.com/${REPO}/releases/tag/${tag}`, pub_date: pubDate, platforms };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [dir, version, tag] = process.argv.slice(2);
  if (!dir || !version || !tag) throw new Error('usage: node scripts/updater-manifest.mjs <dir> <version> <tag>');
  const files = {};
  for (const f of readdirSync(dir)) {
    if (f.endsWith('.sig')) files[f.slice(0, -4)] = { sig: readFileSync(join(dir, f), 'utf8'), size: statSync(join(dir, f.slice(0, -4))).size };
  }
  const manifest = buildManifest({ version, tag, files, pubDate: new Date().toISOString() });
  writeFileSync(join(dir, 'latest.json'), JSON.stringify(manifest, null, 2) + '\n');
  console.log(`latest.json: ${Object.keys(manifest.platforms).join(', ')}`);
}
