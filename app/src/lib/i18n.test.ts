import { beforeEach, describe, expect, it } from 'vitest';
import { en, tr } from './messages';
import { kk } from './messages-kk';
import { ru } from './messages-ru';
import { detectLang, errText, i18n, LANGUAGES, num, setLang, t } from './i18n.svelte';

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

  it('uses Russian one/few/many and the Kazakh singular after numbers', () => {
    setLang('ru');
    expect([1, 2, 5, 21, 22, 25, 11].map((n) => t('imp.files', { n }))).toEqual([
      '1 файл', '2 файла', '5 файлов', '21 файл', '22 файла', '25 файлов', '11 файлов'
    ]);
    expect(t('set.search.count', { n: 3 })).toBe('Найдено 3 настройки');
    setLang('kk');
    expect([1, 5].map((n) => t('imp.files', { n }))).toEqual(['1 файл', '5 файл']);
    setLang('en');
  });

  it('formats numbers for the language', () => {
    expect(num(1.5)).toBe('1.5');
    expect(t('gfx.speedN', { n: 1.5 })).toBe('1.5×');
    expect(num(12345)).toBe('12345');
    setLang('ru');
    expect(num(1.5)).toBe('1,5');
    expect(num(2, 1)).toBe('2,0');
    expect(t('upd.size', { n: 61.3 })).toBe('Размер загрузки 61,3 МБ');
    setLang('tr');
    expect(t('ready.ui', { n: 120 })).toBe('Arayüz %120');
    setLang('en');
  });

  it('picks the system language when the launcher has it', () => {
    expect(LANGUAGES.map((l) => l.label)).toEqual(['English', 'Türkçe', 'Русский', 'Қазақша']);
    expect(detectLang(['ru-RU', 'en-US'])).toBe('ru');
    expect(detectLang(['de-DE', 'kk_KZ'])).toBe('kk');
    expect(detectLang(['TR'])).toBe('tr');
    expect(detectLang(['fr', 'de'])).toBe('en');
    expect(detectLang([])).toBe('en');
  });

  it('keeps every language complete and placeholders and plural cases in step', () => {
    const holes = (s: string) => [...s.matchAll(/\{(\w+)[,}]/g)].map((m) => m[1]).sort().join();
    const cases = (s: string) => [...s.matchAll(/(\w+) \{[^{}]*\}/g)].map((m) => m[1]).join();
    const want: Record<string, string> = { tr: 'one,other', ru: 'one,few,many,other', kk: 'one,other' };
    for (const [code, dict] of Object.entries({ tr, ru, kk })) {
      expect(Object.keys(dict), code).toEqual(Object.keys(en));
      for (const k of Object.keys(en) as (keyof typeof en)[]) {
        expect(holes(dict[k]), `${code} ${k}`).toBe(holes(en[k]));
        if (en[k].includes(', plural,')) expect(cases(dict[k]), `${code} ${k}`).toBe(want[code]);
      }
    }
  });

  it('shows backend failures as text', () => {
    expect(errText('i/o error: denied')).toBe('i/o error: denied');
    expect(errText(new Error('boom'))).toBe('boom');
  });
});
