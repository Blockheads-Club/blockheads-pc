import { en as baseEn, tr as baseTr, type Key } from './messages';
import { kk as baseKk } from './messages-kk';
import { modsEn } from './messages-mods';
import { modsKk } from './messages-mods-kk';
import { modsRu } from './messages-mods-ru';
import { modsTr } from './messages-mods-tr';
import { ru as baseRu } from './messages-ru';

const en: Record<Key, string> = { ...baseEn, ...modsEn };
const tr: Record<Key, string> = { ...baseTr, ...modsTr };
const ru: Record<Key, string> = { ...baseRu, ...modsRu };
const kk: Record<Key, string> = { ...baseKk, ...modsKk };

/** The launcher's languages, each named in itself. */
export const LANGUAGES = [
  { code: 'en', label: 'English' },
  { code: 'tr', label: 'Türkçe' },
  { code: 'ru', label: 'Русский' },
  { code: 'kk', label: 'Қазақша' }
] as const;

const dicts: Record<string, Record<Key, string>> = { en, tr, ru, kk };

/** The first of the system's preferred languages (`ru-RU`, `kk_KZ`, `tr`…) that the launcher has, else English. */
export function detectLang(preferred: readonly string[]): string {
  for (const tag of preferred) {
    const code = tag.toLowerCase().split(/[-_]/)[0];
    if (code in dicts) return code;
  }
  return 'en';
}

export const i18n = $state({ lang: 'en' });

export function setLang(code: string) {
  i18n.lang = code in dicts ? code : 'en';
  document.documentElement.lang = i18n.lang;
}

type Params = Record<string, string | number>;

/** `n` in the language's digits and decimal mark (`1.5` / `1,5`), without thousands grouping. */
export function num(n: number, digits?: number): string {
  const opts = digits === undefined ? { maximumFractionDigits: 20 } : { minimumFractionDigits: digits, maximumFractionDigits: digits };
  return new Intl.NumberFormat(i18n.lang, { useGrouping: false, ...opts }).format(n);
}

/** Resolve `{n, plural, one {# item} other {# items}}` with the language's plural rules. */
function plural(s: string, params: Params): string {
  return s.replace(/\{(\w+), plural,((?:\s*\w+ \{[^{}]*\})+)\s*\}/g, (_, k: string, body: string) => {
    const n = Number(params[k]);
    const cases = Object.fromEntries([...body.matchAll(/(\w+) \{([^{}]*)\}/g)].map((m) => [m[1], m[2]]));
    const rule = new Intl.PluralRules(i18n.lang).select(n);
    return (cases[rule] ?? cases.other).replace(/#/g, num(n));
  });
}

/**
 * Translate; `{name}` placeholders are filled from `params` (numbers in the language's format). Unknown keys fall
 * back to English.
 */
export function t(key: Key, params: Params = {}): string {
  const s = plural(dicts[i18n.lang]?.[key] ?? en[key] ?? key, params);
  return s.replace(/\{(\w+)\}/g, (_, k) => {
    const v = params[k];
    return v === undefined ? `{${k}}` : typeof v === 'number' ? num(v) : v;
  });
}

/** Text of a backend failure: Tauri rejects with the message itself, anything else is an `Error`. */
export function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
