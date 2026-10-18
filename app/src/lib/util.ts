import type { KeyBindings } from './types';

/** Copy text to the clipboard; false when the platform refuses. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Run `fn` with a localStorage that may be missing or throw (private windows, blocked storage). */
export function storage<T>(fn: (s: Storage) => T, fallback: T): T {
  try {
    return fn(localStorage);
  } catch {
    return fallback;
  }
}

/** Last path segment, for either separator. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Binding text as the backend writes it (`Ctrl+Left`), lower-cased for comparisons. */
export const sameBinding = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Pairs of action ids that share a binding. */
export function conflicts(keys: KeyBindings): [keyof KeyBindings, keyof KeyBindings][] {
  const ids = Object.keys(keys) as (keyof KeyBindings)[];
  const out: [keyof KeyBindings, keyof KeyBindings][] = [];
  ids.forEach((a, i) => {
    for (const b of ids.slice(i + 1)) if (sameBinding(keys[a], keys[b])) out.push([a, b]);
  });
  return out;
}

const KEY_NAMES: Record<string, string> = {
  Escape: 'Esc',
  ' ': 'Space',
  ArrowLeft: 'ArrowLeft',
  ArrowRight: 'ArrowRight',
  ArrowUp: 'Up',
  ArrowDown: 'Down'
};
const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'CapsLock']);

function mods(e: { ctrlKey: boolean; shiftKey: boolean; altKey: boolean }): string {
  return (e.ctrlKey ? 'Ctrl+' : '') + (e.shiftKey ? 'Shift+' : '') + (e.altKey ? 'Alt+' : '');
}

/** Binding text for a key press, or null for a bare modifier. */
export function keyToBinding(e: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const name = KEY_NAMES[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key);
  return mods(e) + name;
}

/** Binding text for a mouse button press. */
export function mouseToBinding(e: MouseEvent): string {
  return mods(e) + (['Left', 'Middle', 'Right'][e.button] ?? 'Left');
}

/** Binding text for a wheel turn. */
export function wheelToBinding(e: WheelEvent): string {
  return mods(e) + 'Wheel';
}

export type Chip = { kind: 'key' | 'plus'; text: string } | { kind: 'mouse'; text: string; icon: 'ml' | 'mr' | 'mw' };

/** Split `Ctrl+Left` into the chips the controls table draws. `label` names the mouse parts. */
export function chips(binding: string, label: (m: 'Left' | 'Right' | 'Middle' | 'Wheel') => string): Chip[] {
  const out: Chip[] = [];
  binding.split('+').forEach((part, i) => {
    if (i > 0) out.push({ kind: 'plus', text: '+' });
    const p = part.trim();
    if (p === 'Left') out.push({ kind: 'mouse', text: label('Left'), icon: 'ml' });
    else if (p === 'Right') out.push({ kind: 'mouse', text: label('Right'), icon: 'mr' });
    else if (p === 'Middle') out.push({ kind: 'mouse', text: label('Middle'), icon: 'ml' });
    else if (p === 'Wheel') out.push({ kind: 'mouse', text: label('Wheel'), icon: 'mw' });
    else out.push({ kind: 'key', text: p.replace(/^Arrow/, '') });
  });
  return out;
}

/** Two-digit zero padded clock, for log timestamps. */
export const pad2 = (n: number) => String(n).padStart(2, '0');

/** Window sizes offered in the dropdown, smallest first. */
export const SIZES: [number, number][] = [
  [1024, 768],
  [1280, 720],
  [1600, 900],
  [1920, 1080],
  [2560, 1440]
];

/** The largest standard size that fits the screen with some margin (the "recommended" one). */
export function recommendedSize(screen: { width: number; height: number } | null): [number, number] {
  if (!screen) return [1280, 720];
  const fit = SIZES.filter(([w, h]) => w <= screen.width * 0.8 && h <= screen.height * 0.8);
  return fit.length ? fit[fit.length - 1] : SIZES[0];
}
