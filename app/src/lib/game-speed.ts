import { i18n, t } from './i18n.svelte';

/** Game speeds offered in the dropdown (matches `GAME_SPEEDS` in crates/bhpc-launcher/src/window.rs). */
export const GAME_SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3];

/** `1.5×` in the UI language's number format. */
export function gameSpeedLabel(n: number): string {
  return t(n === 1 ? 'gfx.speedNormal' : 'gfx.speedN', { n: n.toLocaleString(i18n.lang) });
}
