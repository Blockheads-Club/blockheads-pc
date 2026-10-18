import { i18n, t } from './i18n.svelte';

/** `1 h 04 min` / `12 min` / `under 1 min` for a duration in seconds. */
export function duration(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  if (h > 0) return t('time.hm', { h, m: String(m).padStart(2, '0') });
  if (m > 0) return t('time.m', { m });
  return t('time.lt1');
}

/** `61.3 MB` with the language's number format. */
export function bytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB'];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  const fmt = new Intl.NumberFormat(i18n.lang, { maximumFractionDigits: u < 1 ? 0 : 1 });
  return `${fmt.format(v)} ${units[u]}`;
}
