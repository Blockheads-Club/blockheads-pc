/**
 * Controller support for the Mods area (design: Controller, keyboard, mouse). The launcher has no game running
 * behind it here, so the pad drives the page like a keyboard: D-pad/left stick move the focus (arrow keys inside
 * lists and tabs, else Tab order), A presses, X toggles a switch or box, B is Esc, LB/RB switch tabs.
 */

export interface PadHandlers {
  /** LB (-1) / RB (+1). */
  tab: (step: -1 | 1) => void;
}

/** Standard mapping button indices. */
const A = 0, B = 1, X = 2, LB = 4, RB = 5, UP = 12, DOWN = 13, LEFT = 14, RIGHT = 15;
const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

function key(name: string) {
  const target = document.activeElement as HTMLElement;
  return target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true }));
}

/** Moves the focus to the next/previous focusable element (inside an open dialog when there is one). */
export function moveFocus(step: 1 | -1) {
  const scope = document.querySelector<HTMLElement>('[aria-modal="true"]') ?? document.body;
  const all = [...scope.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((e) => e.offsetParent !== null);
  if (!all.length) return;
  const i = all.indexOf(document.activeElement as HTMLElement);
  all[i < 0 ? 0 : (i + step + all.length) % all.length].focus();
}

/** Arrow keys first (lists, tabs, radio groups use them); when nothing handled one, move along the Tab order. */
function direction(arrow: string, step: 1 | -1) {
  if (key(arrow)) moveFocus(step);
}

export function press(button: number, h: PadHandlers) {
  const el = document.activeElement as HTMLElement | null;
  if (button === A) el?.click();
  else if (button === X && el instanceof HTMLInputElement && el.type === 'checkbox') el.click();
  else if (button === B) key('Escape');
  else if (button === LB) h.tab(-1);
  else if (button === RB) h.tab(1);
  else if (button === UP) direction('ArrowUp', -1);
  else if (button === DOWN) direction('ArrowDown', 1);
  else if (button === LEFT) direction('ArrowLeft', -1);
  else if (button === RIGHT) direction('ArrowRight', 1);
}

/** Stick to D-pad: a push past half way counts as the button. */
function stick(p: Gamepad): number[] {
  const [x = 0, y = 0] = p.axes;
  return [y < -0.5 && UP, y > 0.5 && DOWN, x < -0.5 && LEFT, x > 0.5 && RIGHT].filter((b): b is number => b !== false);
}

/** Polls the connected pads every frame and calls `press` on each new press; returns the stop function. */
export function watchPad(h: PadHandlers): () => void {
  if (typeof navigator.getGamepads !== 'function') return () => {};
  let held = new Set<string>();
  let raf = 0;
  const tick = () => {
    const now = new Set<string>();
    for (const p of navigator.getGamepads()) {
      if (!p) continue;
      const down = [...p.buttons.flatMap((b, i) => (b.pressed ? [i] : [])), ...stick(p)];
      for (const b of down) {
        const id = `${p.index}:${b}`;
        now.add(id);
        if (!held.has(id)) press(b, h);
      }
    }
    held = now;
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(raf);
}
