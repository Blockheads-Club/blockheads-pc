import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('$lib/api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import { makeApi, makeBoot, makeSnap, makeWin } from '$lib/mock-api';
import { store } from '$lib/store.svelte';
import { resetStore } from '$lib/test-utils';
import { theme } from '$lib/theme.svelte';
import { viewport } from '$lib/viewport.svelte';
import Layout from './+layout.svelte';
import * as load from './+layout';
import Page from './+page.svelte';

let hooks: ReturnType<typeof makeWin>['hooks'];
const listeners: (() => void)[] = [];
const flags = { compact: false, narrow: false };
function mq(next: Partial<typeof flags>) {
  Object.assign(flags, { compact: false, narrow: false }, next);
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      get matches() {
        return query.includes('max-width: 599px') ? flags.narrow : flags.compact;
      },
      addEventListener: (_: string, l: () => void) => listeners.push(l),
      removeEventListener: vi.fn()
    })
  });
}

beforeEach(() => {
  mock.api = makeApi();
  const w = makeWin();
  mock.win = w.win;
  hooks = w.hooks;
  resetStore({ screen: 'splash', config: null });
  viewport.compact = false;
  mq({});
});
afterEach(() => {
  cleanup();
  store.stopPolling();
});

describe('layout', () => {
  it('is a client-only app', () => {
    expect(load.ssr).toBe(false);
    expect(load.prerender).toBe(false);
  });

  it('renders its children', () => {
    render(Layout, { children: (() => {}) as never });
  });
});

describe('page', () => {
  it('keeps the window empty while booting, feeds the splash window, then reveals Home', async () => {
    mock.api.boot = vi.fn(async () => makeBoot());
    render(Page);
    expect(screen.queryByRole('status')).toBeNull();
    await waitFor(() => expect(mock.api.splashSet).toHaveBeenCalledWith(expect.objectContaining({ boot: 'loading', lang: 'en' })));
    expect(mock.api.mainReady).not.toHaveBeenCalled();
    expect(await screen.findByRole('heading', { name: /The Blockheads/ })).toBeVisible();
    expect(document.title).toBe('Blockheads PC · Home');
    expect(mock.api.mainReady).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute('href', '#main');
    expect(theme.choice).toBe('system');
  });

  it('opens the setup wizard on the first run and walks its steps', async () => {
    mock.api.boot = vi.fn(async () => makeBoot({ config: null }));
    render(Page);
    expect(await screen.findByRole('heading', { name: 'Play The Blockheads on your computer' })).toBeVisible();
    expect(document.title).toBe('Blockheads PC · Setup · Step 1 of 6');
    store.legalOk = true;
    for (const [n, heading] of [
      [2, 'Choose your game file'], [3, 'Window and graphics'], [4, 'Controls'], [5, 'Bring your worlds from your phone'], [6, 'You’re ready to play!']
    ] as const) {
      if (n === 3) store.apk = { path: '/a', size_bytes: 1, status: 'supported', version: '1.7.6', problem: null, detail: '' };
      store.next();
      expect(await screen.findByRole('heading', { name: heading })).toBeVisible();
    }
  });

  it('moves focus to the main region when the screen or step changes', async () => {
    mock.api.boot = vi.fn(async () => makeBoot({ config: null }));
    render(Page);
    await screen.findByRole('heading', { name: 'Play The Blockheads on your computer' });
    expect(document.getElementById('main')).not.toHaveFocus();
    store.legalOk = true;
    store.next();
    await screen.findByRole('heading', { name: 'Choose your game file' });
    await waitFor(() => expect(document.getElementById('main')).toHaveFocus());
  });

  it('uses the shortcuts of the design', async () => {
    mock.api.boot = vi.fn(async () => makeBoot());
    let started = false;
    mock.api.gameStart = vi.fn(async () => { started = true; });
    mock.api.gamePoll = vi.fn(async () => (started ? makeSnap({ state: 'starting', running_secs: 0 }) : makeSnap()));
    render(Page);
    await screen.findByRole('heading', { name: /The Blockheads/ });
    // F1 and Ctrl+L open dialogs, Ctrl+L again closes the log.
    await fireEvent.keyDown(window, { key: 'F1' });
    expect(store.dialog).toBe('join');
    expect(document.title).toBe('Blockheads PC · Help');
    store.dialog = null;
    await fireEvent.keyDown(window, { key: 'l', ctrlKey: true });
    expect(store.dialog).toBe('logs');
    await fireEvent.keyDown(window, { key: 'L', metaKey: true });
    expect(store.dialog).toBeNull();
    // Enter on Home plays, but not inside a dialog or on a control.
    store.dialog = 'join';
    await fireEvent.keyDown(document.body, { key: 'Enter' });
    expect(mock.api.gameStart).not.toHaveBeenCalled();
    store.dialog = null;
    await fireEvent.keyDown(screen.getByRole('button', { name: 'Settings' }), { key: 'Enter' });
    expect(mock.api.gameStart).not.toHaveBeenCalled();
    await fireEvent.keyDown(document.body, { key: 'Enter', ctrlKey: true });
    expect(mock.api.gameStart).not.toHaveBeenCalled();
    await fireEvent.keyDown(document.body, { key: 'Enter' });
    await waitFor(() => expect(mock.api.gameStart).toHaveBeenCalled());
    await waitFor(() => expect(store.screen).toBe('playing'));
    expect(document.title).toBe('Blockheads PC · Playing The Blockheads 1.7.6');
    store.screen = 'home';
    // Ctrl+, opens Settings; Enter is ignored while the game runs.
    await fireEvent.keyDown(window, { key: ',', ctrlKey: true });
    expect(store.screen).toBe('settings');
    expect(document.title).toBe('Blockheads PC · Settings');
    store.section = 'controls';
    await waitFor(() => expect(document.title).toBe('Blockheads PC · Settings · Controls'));
    store.section = 'about';
    await waitFor(() => expect(document.title).toBe('Blockheads PC · Settings · About'));
    expect(await screen.findByText('Updates')).toBeVisible();
    store.section = 'controls';
    expect(await screen.findByRole('heading', { name: 'Controls' })).toBeVisible();
    store.screen = 'home';
    store.game = makeSnap({ state: 'running', running_secs: 5 });
    await fireEvent.keyDown(document.body, { key: 'Enter' });
    expect(mock.api.gameStart).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Back to game' })).toBeVisible();
  });

  it('ignores shortcuts that need a saved configuration, and handles setup keys', async () => {
    mock.api.boot = vi.fn(async () => makeBoot({ config: null }));
    render(Page);
    await screen.findByRole('heading', { name: 'Play The Blockheads on your computer' });
    for (const k of [{ key: ',', ctrlKey: true }, { key: 'l', ctrlKey: true }, { key: 'F1' }]) await fireEvent.keyDown(window, k);
    expect(store.dialog).toBeNull();
    store.legalOk = true;
    await fireEvent.keyDown(window, { key: 'ArrowRight', altKey: true });
    expect(store.step).toBe(2);
    await fireEvent.keyDown(window, { key: 'ArrowLeft', altKey: true });
    expect(store.step).toBe(1);
    const e = new KeyboardEvent('keydown', { key: 'F1', cancelable: true });
    e.preventDefault();
    window.dispatchEvent(e);
    await fireEvent.keyDown(window, { key: 'x' });
  });

  it('shows every dialog from the store', async () => {
    mock.api.boot = vi.fn(async () => makeBoot());
    render(Page);
    await screen.findByRole('heading', { name: /The Blockheads/ });
    for (const [d, name] of [
      ['join', 'How to join a server'], ['logs', 'Game log'], ['import', 'Bring your worlds from your phone'],
      ['close', 'Stop the game and quit Blockheads PC?']
    ] as const) {
      store.dialog = d;
      expect(await screen.findByRole(d === 'close' ? 'alertdialog' : 'dialog', { name })).toBeVisible();
    }
    store.update.stage = 'available';
    store.update.info = { version: '1', size_mb: 1, notes: [] };
    store.dialog = 'update';
    expect(await screen.findByRole('dialog', { name: /is here/ })).toBeVisible();
  });

  it('asks before quitting from the tray while the game runs, and labels the tray menu', async () => {
    let quit: (() => void) | undefined;
    mock.api.onQuitRequested = vi.fn(async (cb: () => void) => { quit = cb; return () => {}; });
    mock.api.boot = vi.fn(async () => makeBoot());
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', running_secs: 1 }));
    render(Page);
    await screen.findByRole('heading', { name: /The Blockheads/ });
    await waitFor(() => expect(quit).toBeDefined());
    await waitFor(() => expect(store.state).toBe('running'));
    quit?.();
    expect(await screen.findByRole('alertdialog')).toBeVisible();
    expect(mock.api.trayLabels).toHaveBeenCalledWith('Show Blockheads PC', 'Quit');
  });

  it('rounds the window except maximized, fullscreen or native (macOS)', async () => {
    mock.api.boot = vi.fn(async () => makeBoot());
    const { container } = render(Page);
    const frame = () => container.querySelector('.frame') as HTMLElement;
    await waitFor(() => expect(mock.win.isFullscreen).toHaveBeenCalled());
    expect(frame().classList.contains('round')).toBe(true);
    mock.win.isMaximized = vi.fn(async () => true);
    hooks.resize?.();
    await waitFor(() => expect(frame().classList.contains('round')).toBe(false));
    mock.win.isMaximized = vi.fn(async () => false);
    mock.win.isFullscreen = vi.fn(async () => true);
    hooks.resize?.();
    await waitFor(() => expect(frame().classList.contains('round')).toBe(false));
    mock.win.isFullscreen = vi.fn(async () => { throw new Error('no window'); });
    hooks.resize?.();
    await new Promise((r) => setTimeout(r, 0));
    expect(frame().classList.contains('round')).toBe(false);
  });

  it('hides the bars of a fullscreen in-window game, F11 toggles it, and the window is square while it is up', async () => {
    mock.api.boot = vi.fn(async () => makeBoot());
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', running_secs: 1, embedded: true }));
    const { container } = render(Page);
    await waitFor(() => expect(store.embedded).toBe(true));
    store.screen = 'playing';
    const frame = container.querySelector('.frame') as HTMLElement;
    expect(frame.classList.contains('round')).toBe(false);
    await fireEvent.keyDown(window, { key: 'F11' });
    expect(mock.api.embedFullscreen).toHaveBeenCalledWith(true);
    mock.win.isFullscreen = vi.fn(async () => true);
    hooks.resize?.();
    await waitFor(() => expect(store.fullscreen).toBe(true));
    expect((await screen.findAllByText('Fullscreen — press F11 to exit')).length).toBeGreaterThan(0);
    // without a saved configuration the toast names F11
    store.fullscreen = false;
    await tick();
    const cfg = store.config;
    store.config = { ...cfg!, keys: { ...cfg!.keys, fullscreen: '' } };
    store.fullscreen = true;
    await tick();
    store.config = cfg;
    await waitFor(() => expect(screen.queryByLabelText('Window title bar')).toBeNull());
    store.screen = 'home';
    await waitFor(() => expect(screen.getByLabelText('Window title bar')).toBeVisible());
    store.screen = 'playing';
    await fireEvent.keyDown(window, { key: 'F11' });
    expect(mock.api.embedFullscreen).toHaveBeenLastCalledWith(false);
  });

  it('F11 makes the launcher window fullscreen when no game view is shown', async () => {
    mock.api.boot = vi.fn(async () => makeBoot());
    render(Page);
    await screen.findByRole('heading', { name: /The Blockheads/ });
    await fireEvent.keyDown(window, { key: 'F11' });
    expect(mock.win.setFullscreen).toHaveBeenCalledWith(true);
  });

  it('survives a window without close events and picks the OS from the user agent', async () => {
    mock.api.onQuitRequested = vi.fn(async () => { throw new Error('no window'); });
    mock.api.trayLabels = vi.fn(async () => { throw new Error('no tray'); });
    mock.win.onResized = vi.fn(async () => { throw new Error('no window'); });
    mock.api.boot = vi.fn(async () => makeBoot());
    mock.api.appInfo = vi.fn(async () => { throw 'denied'; });
    const ua = vi.spyOn(navigator, 'userAgent', 'get');
    for (const [agent, label] of [['Mozilla (Windows NT 10)', 'Minimize window'], ['Mozilla (Macintosh)', 'Close window'], ['X11 Linux', 'Close window']] as const) {
      ua.mockReturnValue(agent);
      resetStore({ screen: 'splash', config: null, info: null });
      const { unmount } = render(Page);
      expect(screen.getByRole('button', { name: label })).toBeVisible();
      await waitFor(() => expect(store.boot).toBe('error'));
      unmount();
    }
    ua.mockRestore();
  });

  it('keeps the OS-drawn controls on macOS and follows the viewport', async () => {
    mq({ compact: true });
    mock.api.boot = vi.fn(async () => makeBoot());
    mock.api.appInfo = vi.fn(async () => ({ version: '1', os: 'macos', arch: 'aarch64', config_dir: '/c', updates_enabled: false, native_controls: true }));
    render(Page);
    await screen.findByRole('heading', { name: /The Blockheads/ });
    expect(viewport.compact).toBe(true);
    expect(screen.queryByRole('button', { name: 'Close window' })).toBeNull();
    expect(screen.getByRole('navigation', { name: 'Quick actions' })).toBeVisible();
  });
});

describe('boot hand-over', () => {
  it('stays hidden on a boot error and retries when the splash asks', async () => {
    let retry: () => void = () => {};
    mock.api.onSplashRetry = vi.fn(async (cb: () => void) => { retry = cb; return () => {}; });
    mock.api.boot = vi.fn(async () => { throw 'denied'; });
    render(Page);
    await waitFor(() => expect(mock.api.splashSet).toHaveBeenCalledWith(expect.objectContaining({ boot: 'error', error: 'denied' })));
    expect(mock.api.mainReady).not.toHaveBeenCalled();
    mock.api.boot = vi.fn(async () => makeBoot());
    retry();
    await screen.findByRole('heading', { name: /The Blockheads/ });
    expect(mock.api.mainReady).toHaveBeenCalledTimes(1);
  });

  it('ignores IPC failures of the hand-over', async () => {
    mock.api.splashSet = vi.fn(async () => { throw 'x'; });
    mock.api.mainReady = vi.fn(async () => { throw 'x'; });
    mock.api.onSplashRetry = vi.fn(async () => { throw 'x'; });
    render(Page);
    await screen.findByRole('heading', { name: /The Blockheads/ });
    expect(mock.api.mainReady).toHaveBeenCalled();
  });
});
