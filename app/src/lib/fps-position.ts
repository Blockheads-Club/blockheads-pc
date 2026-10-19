import { t } from './i18n.svelte';
import type { FpsPosition } from './types';

/** The FPS counter corners, in the order the settings list them. */
export const FPS_POSITIONS: FpsPosition[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right'];

const KEYS = {
  'top-left': 'gfx.fpsPos.topLeft',
  'top-right': 'gfx.fpsPos.topRight',
  'bottom-left': 'gfx.fpsPos.bottomLeft',
  'bottom-right': 'gfx.fpsPos.bottomRight'
} as const;

/** The translated name of a corner. */
export function fpsPositionLabel(p: FpsPosition): string {
  return t(KEYS[p]);
}
