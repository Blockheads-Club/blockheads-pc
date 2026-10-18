import { beforeEach, describe, expect, it } from 'vitest';
import { en, tr } from './messages';
import { errText, i18n, setLang, t } from './i18n.svelte';

describe('i18n', () => {
  beforeEach(() => setLang('en'));

  it('defaults to English and interpolates', () => {
    expect(t('common.back')).toBe('Back');
    expect(t('tb.setup', { n: 2, total: 6 })).toBe('Setup · Step 2 of 6');
    expect(t('tb.setup')).toBe('Setup · Step {n} of {total}');
  });

  it('switches to Turkish and falls back for unknown languages', () => {
    setLang('tr');
    expect(t('common.back')).toBe('Geri');
    expect(document.documentElement.lang).toBe('tr');
    setLang('xx');
    expect(i18n.lang).toBe('en');
  });

  it('falls back to English or the key when a language lacks a message', () => {
    i18n.lang = 'zz';
    expect(t('common.back')).toBe('Back');
    expect(t('nope' as never)).toBe('nope');
  });

  it('resolves plural forms per language', () => {
    expect(t('imp.files', { n: 1 })).toBe('1 file');
    expect(t('imp.files', { n: 3 })).toBe('3 files');
    setLang('tr');
    expect(t('imp.files', { n: 3 })).toBe('3 dosya');
    i18n.lang = 'ar';
    expect(t('imp.files', { n: 2 })).toBe('2 files');
    setLang('en');
  });

  it('keeps both languages complete and placeholders in step', () => {
    expect(Object.keys(tr).sort()).toEqual(Object.keys(en).sort());
    const holes = (s: string) => [...s.matchAll(/\{(\w+)[,}]/g)].map((m) => m[1]).sort().join();
    for (const k of Object.keys(en) as (keyof typeof en)[]) expect(holes(tr[k]), k).toBe(holes(en[k]));
  });

  it('shows backend failures as text', () => {
    expect(errText('i/o error: denied')).toBe('i/o error: denied');
    expect(errText(new Error('boom'))).toBe('boom');
  });
});
