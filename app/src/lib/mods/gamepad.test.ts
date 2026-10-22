import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { moveFocus, press, watchPad } from './gamepad';

const pad = (pressed: number[], axes = [0, 0], index = 0) =>
  ({ index, axes, buttons: Array.from({ length: 16 }, (_, i) => ({ pressed: pressed.includes(i) })) }) as unknown as Gamepad;

describe('controller', () => {
  let frames: (() => void)[] = [];
  beforeEach(() => {
    frames = [];
    vi.stubGlobal('requestAnimationFrame', (cb: () => void) => frames.push(cb));
    vi.stubGlobal('cancelAnimationFrame', vi.fn());
    document.body.innerHTML = '<button id="a">A</button><input id="c" type="checkbox"><input id="t"><button id="b">B</button>';
    // jsdom has no layout: every element counts as shown.
    for (const el of document.querySelectorAll<HTMLElement>('*')) Object.defineProperty(el, 'offsetParent', { value: document.body, configurable: true });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = '';
  });

  it('moves the focus along the page or inside a dialog', () => {
    const hidden = document.createElement('button');
    document.body.prepend(hidden);
    moveFocus(1);
    expect(document.activeElement?.id).toBe('a');
    moveFocus(1);
    expect(document.activeElement?.id).toBe('c');
    moveFocus(-1);
    moveFocus(-1);
    expect(document.activeElement?.id).toBe('b');
    document.body.innerHTML = '<div aria-modal="true"></div>';
    moveFocus(1);
  });

  it('maps the buttons', () => {
    const tab = vi.fn();
    const a = document.getElementById('a')!;
    const clicked = vi.fn();
    a.addEventListener('click', clicked);
    a.focus();
    press(0, { tab });
    expect(clicked).toHaveBeenCalled();
    press(2, { tab });
    const c = document.getElementById('c') as HTMLInputElement;
    c.focus();
    press(2, { tab });
    expect(c.checked).toBe(true);
    (document.getElementById('t') as HTMLElement).focus();
    press(2, { tab });
    const esc = vi.fn();
    document.addEventListener('keydown', esc);
    press(1, { tab });
    expect(esc.mock.calls[0][0].key).toBe('Escape');
    press(4, { tab });
    press(5, { tab });
    expect(tab.mock.calls).toEqual([[-1], [1]]);
    // Arrows go to the focused element first; unhandled ones move the focus.
    a.focus();
    press(13, { tab });
    expect(document.activeElement?.id).toBe('c');
    press(12, { tab });
    expect(document.activeElement?.id).toBe('a');
    press(15, { tab });
    press(14, { tab });
    expect(document.activeElement?.id).toBe('a');
    const stop = (e: KeyboardEvent) => e.preventDefault();
    document.addEventListener('keydown', stop);
    press(13, { tab });
    expect(document.activeElement?.id).toBe('a');
    document.removeEventListener('keydown', stop);
    document.removeEventListener('keydown', esc);
    press(9, { tab });
    (document.activeElement as HTMLElement).blur();
    press(0, { tab });
  });

  it('polls the pads and acts on new presses only', () => {
    const tab = vi.fn();
    let pads: (Gamepad | null)[] = [null, pad([5]), pad([], [0.9, -0.9], 2)];
    Object.defineProperty(navigator, 'getGamepads', { value: () => pads, configurable: true });
    const stop = watchPad({ tab });
    frames.shift()!();
    expect(tab).toHaveBeenCalledTimes(1);
    frames.shift()!();
    expect(tab).toHaveBeenCalledTimes(1);
    pads = [pad([]), pad([], [-0.9, 0.9], 2)];
    frames.shift()!();
    pads = [pad([4]), pad([], [], 3)];
    frames.shift()!();
    expect(tab).toHaveBeenLastCalledWith(-1);
    stop();
    expect(cancelAnimationFrame).toHaveBeenCalled();
    Object.defineProperty(navigator, 'getGamepads', { value: undefined, configurable: true });
    watchPad({ tab })();
  });
});
