// SPDX-License-Identifier: GPL-3.0-or-later
// node --test scripts/updater-manifest.test.mjs
import assert from 'node:assert/strict';
import test from 'node:test';
import { PLATFORMS, buildManifest } from './updater-manifest.mjs';

const files = Object.fromEntries(
  Object.values(PLATFORMS).map((s, i) => [`Blockheads.PC_0.2.0${s}`, { sig: ` sig${i}\n`, size: 10 + i }])
);

test('one entry per platform with signature, url and size', () => {
  const m = buildManifest({ version: '0.2.0', tag: 'v0.2.0', files, pubDate: 'now' });
  assert.deepEqual(Object.keys(m.platforms), Object.keys(PLATFORMS));
  assert.deepEqual(m.platforms['linux-x86_64'], {
    signature: 'sig2',
    url: 'https://github.com/Blockheads-Club/blockheads-pc/releases/download/v0.2.0/Blockheads.PC_0.2.0_amd64.AppImage',
    size: 12
  });
  assert.equal(m.version, '0.2.0');
  assert.match(m.notes, /releases\/tag\/v0\.2\.0$/);
});

test('refuses a partial feed', () => {
  delete files['Blockheads.PC_0.2.0_x64-setup.exe'];
  assert.throws(() => buildManifest({ version: '0.2.0', tag: 'v0.2.0', files, pubDate: 'now' }), /windows-x86_64/);
});
