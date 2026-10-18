import { beforeEach, describe, expect, it } from 'vitest';
import { bytes, duration } from './format';
import { setLang } from './i18n.svelte';

describe('format', () => {
  beforeEach(() => setLang('en'));

  it('formats durations', () => {
    expect(duration(3840)).toBe('1 h 04 min');
    expect(duration(720)).toBe('12 min');
    expect(duration(20)).toBe('under 1 min');
    expect(duration(-5)).toBe('under 1 min');
  });

  it('formats sizes with the language rules', () => {
    expect(bytes(512)).toBe('512 B');
    expect(bytes(2048)).toBe('2 KB');
    expect(bytes(1536)).toBe('1.5 KB');
    expect(bytes(64_300_000)).toBe('61.3 MB');
    expect(bytes(3 * 1024 ** 3)).toBe('3 GB');
    expect(bytes(1.5 * 1024 ** 3)).toBe('1.5 GB');
    setLang('tr');
    expect(bytes(1.5 * 1024 ** 3)).toBe('1,5 GB');
  });
});
