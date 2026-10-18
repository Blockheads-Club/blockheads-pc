import { beforeEach, describe, expect, it, vi } from 'vitest';
import { effectiveDark, initTheme, setTheme, theme } from './theme.svelte';

describe('theme', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
  });

  it('defaults to following the system', () => {
    initTheme();
    expect(theme.choice).toBe('system');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('persists and restores an explicit choice', () => {
    setTheme('light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    theme.choice = 'system';
    initTheme();
    expect(theme.choice).toBe('light');
    setTheme('system');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('restores dark and answers whether the UI is dark', () => {
    localStorage.setItem('bhpc.theme', 'dark');
    initTheme();
    expect(theme.choice).toBe('dark');
    expect(effectiveDark()).toBe(true);
    setTheme('light');
    expect(effectiveDark()).toBe(false);
    setTheme('system');
    expect(effectiveDark()).toBe(false);
    const mq = vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: true } as MediaQueryList);
    expect(effectiveDark()).toBe(true);
    mq.mockRestore();
  });

  it('ignores junk and blocked storage', () => {
    localStorage.setItem('bhpc.theme', 'neon');
    initTheme();
    expect(theme.choice).toBe('system');
    const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
    setTheme('dark');
    expect(theme.choice).toBe('dark');
    spy.mockRestore();
  });
});
