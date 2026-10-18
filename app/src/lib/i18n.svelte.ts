import { en, tr, type Key } from './messages';

export const LANGUAGES = [
  { code: 'en', label: 'English' },
  { code: 'tr', label: 'Türkçe' }
] as const;

const dicts: Record<string, Record<Key, string>> = { en, tr };

export const i18n = $state({ lang: 'en' });

export function setLang(code: string) {
  i18n.lang = code in dicts ? code : 'en';
  document.documentElement.lang = i18n.lang;
}

type Params = Record<string, string | number>;

/** Resolve `{n, plural, one {# item} other {# items}}` with the language's plural rules. */
function plural(s: string, params: Params): string {
  return s.replace(/\{(\w+), plural,((?:\s*\w+ \{[^{}]*\})+)\s*\}/g, (_, k: string, body: string) => {
    const n = Number(params[k]);
    const cases = Object.fromEntries([...body.matchAll(/(\w+) \{([^{}]*)\}/g)].map((m) => [m[1], m[2]]));
    const rule = new Intl.PluralRules(i18n.lang).select(n);
    return (cases[rule] ?? cases.other).replace(/#/g, String(n));
  });
}

/** Translate; `{name}` placeholders are filled from `params`. Unknown keys fall back to English. */
export function t(key: Key, params: Params = {}): string {
  const s = plural(dicts[i18n.lang]?.[key] ?? en[key] ?? key, params);
  return s.replace(/\{(\w+)\}/g, (_, k) => String(params[k] ?? `{${k}}`));
}

/** Text of a backend failure: Tauri rejects with the message itself, anything else is an `Error`. */
export function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
