import { describe, expect, it, vi } from 'vitest';
import { KEYS } from './mock-api';
import {
  baseName, chips, conflicts, copyText, keyToBinding, mouseToBinding, pad2, recommendedSize, sameBinding, SIZES,
  storage, wheelToBinding
} from './util';

const key = (k: string, mods: Partial<KeyboardEvent> = {}) => ({ key: k, ctrlKey: false, shiftKey: false, altKey: false, ...mods }) as KeyboardEvent;

describe('util', () => {
  it('copies text and reports a refusal', async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    expect(await copyText('x')).toBe(true);
    writeText.mockRejectedValueOnce(new Error('no'));
    expect(await copyText('x')).toBe(false);
  });

  it('survives missing storage', () => {
    expect(storage((s) => s.length, -1)).toBeGreaterThanOrEqual(0);
    expect(storage(() => { throw new Error('blocked'); }, 'fallback')).toBe('fallback');
  });

  it('takes file names from both kinds of path', () => {
    expect(baseName('/a/b/c.apk')).toBe('c.apk');
    expect(baseName('C:\\Users\\m\\g.apk')).toBe('g.apk');
    expect(baseName('')).toBe('');
  });

  it('finds clashing bindings case-insensitively', () => {
    expect(sameBinding('Ctrl+Left', 'ctrl+left')).toBe(true);
    expect(conflicts(KEYS)).toEqual([]);
    expect(conflicts({ ...KEYS, pan: 'left' })).toEqual([['touch', 'pan']]);
  });

  it('turns input events into binding text', () => {
    expect(keyToBinding(key('Escape'))).toBe('Esc');
    expect(keyToBinding(key(' '))).toBe('Space');
    expect(keyToBinding(key('ArrowUp'))).toBe('Up');
    expect(keyToBinding(key('ArrowLeft'))).toBe('ArrowLeft');
    expect(keyToBinding(key('q', { ctrlKey: true, shiftKey: true, altKey: true }))).toBe('Ctrl+Shift+Alt+Q');
    expect(keyToBinding(key('F5'))).toBe('F5');
    expect(keyToBinding(key('Shift'))).toBeNull();
    expect(keyToBinding(key('Control'))).toBeNull();
    const m = (button: number, ctrlKey = false) => ({ button, ctrlKey, shiftKey: false, altKey: false }) as MouseEvent;
    expect(mouseToBinding(m(0))).toBe('Left');
    expect(mouseToBinding(m(1))).toBe('Middle');
    expect(mouseToBinding(m(2, true))).toBe('Ctrl+Right');
    expect(mouseToBinding(m(7))).toBe('Left');
    expect(wheelToBinding({ ctrlKey: false, shiftKey: true, altKey: false } as WheelEvent)).toBe('Shift+Wheel');
  });

  it('splits bindings into chips', () => {
    const label = (m: string) => m.toLowerCase();
    expect(chips('Ctrl+Left', label)).toEqual([
      { kind: 'key', text: 'Ctrl' },
      { kind: 'plus', text: '+' },
      { kind: 'mouse', text: 'left', icon: 'ml' }
    ]);
    expect(chips('Right', label)[0]).toMatchObject({ icon: 'mr' });
    expect(chips('Middle', label)[0]).toMatchObject({ icon: 'ml' });
    expect(chips('Wheel', label)[0]).toMatchObject({ icon: 'mw' });
    expect(chips('ArrowLeft', label)).toEqual([{ kind: 'key', text: 'Left' }]);
  });

  it('pads, and picks a recommended window size', () => {
    expect(pad2(5)).toBe('05');
    expect(recommendedSize(null)).toEqual([1280, 720]);
    expect(recommendedSize({ width: 2560, height: 1440 })).toEqual([1920, 1080]);
    expect(recommendedSize({ width: 1366, height: 768 })).toEqual([1024, 768]);
    expect(recommendedSize({ width: 800, height: 600 })).toEqual(SIZES[0]);
  });
});
