import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const cap = JSON.parse(readFileSync('src-tauri/capabilities/default.json', 'utf8')) as { permissions: (string | { identifier: string })[] };
const granted = cap.permissions.map((p) => (typeof p === 'string' ? p : p.identifier));

describe('window capability', () => {
  // Every call in api.ts `win` (and the drag region) needs its permission, or the button silently does nothing.
  // onCloseRequested() ends in destroy() when the handler does not prevent the close: without allow-destroy the
  // close button is dead.
  it.each([
    'minimize', 'toggle-maximize', 'close', 'destroy', 'is-maximized', 'is-fullscreen', 'start-dragging', 'start-resize-dragging'
  ])('grants core:window:allow-%s', (name) => {
    expect(granted).toContain(`core:window:allow-${name}`);
  });
});
