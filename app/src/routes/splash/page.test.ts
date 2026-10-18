import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any> }));
vi.mock('$lib/api', () => ({ api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }) }));

import { makeApi } from '$lib/mock-api';
import type { SplashState } from '$lib/types';
import Page from './+page.svelte';

const st = (over: Partial<SplashState> = {}): SplashState => ({
  boot: 'loading', error: '', first_run: false, version: '1.2.3', config_dir: '/cfg', lang: 'en', ...over
});

beforeEach(() => {
  mock.api = makeApi();
});
afterEach(() => cleanup());

describe('splash window page', () => {
  it('is empty until the main window has sent a state, then follows events', async () => {
    let push: (s: SplashState) => void = () => {};
    mock.api.onSplashState = vi.fn(async (cb: (s: SplashState) => void) => { push = cb; return () => {}; });
    render(Page);
    expect(screen.queryByRole('status')).toBeNull();
    await waitFor(() => expect(mock.api.onSplashState).toHaveBeenCalled());
    push(st({ lang: 'tr' }));
    expect(await screen.findByRole('status', { name: 'Blockheads PC başlıyor' })).toBeVisible();
  });

  it('picks up the state that was set before it opened, and wires its buttons', async () => {
    mock.api.splashGet = vi.fn(async () => st({ boot: 'error', error: 'denied' }));
    render(Page);
    expect(await screen.findByRole('alert')).toBeVisible();
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Open folder' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Quit' }));
    expect(mock.api.splashRetry).toHaveBeenCalled();
    expect(mock.api.openPath).toHaveBeenCalledWith('/cfg');
    expect(mock.api.appQuit).toHaveBeenCalled();
  });

  it('prefers a pushed state over the stored one and survives IPC errors', async () => {
    let push: (s: SplashState) => void = () => {};
    let release: (s: SplashState | null) => void = () => {};
    mock.api.onSplashState = vi.fn(async (cb: (s: SplashState) => void) => { push = cb; return () => {}; });
    mock.api.splashGet = vi.fn(() => new Promise((r) => (release = r)));
    render(Page);
    await waitFor(() => expect(mock.api.onSplashState).toHaveBeenCalled());
    push(st({ first_run: true }));
    release(st());
    await screen.findByRole('status', { name: 'Blockheads PC setup is starting' });
    cleanup();
    mock.api.onSplashState = vi.fn(async () => { throw 'x'; });
    mock.api.splashGet = vi.fn(async () => { throw 'x'; });
    render(Page);
    await waitFor(() => expect(mock.api.splashGet).toHaveBeenCalled());
  });

  it('releases the listener it registered, even if unmounted first', async () => {
    const off = vi.fn();
    mock.api.onSplashState = vi.fn(async () => off);
    const a = render(Page);
    await waitFor(() => expect(mock.api.onSplashState).toHaveBeenCalled());
    await Promise.resolve();
    a.unmount();
    expect(off).toHaveBeenCalled();
    const b = render(Page);
    b.unmount();
    await waitFor(() => expect(off).toHaveBeenCalledTimes(2));
  });

  it('uses square corners on macOS', async () => {
    const ua = vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla (Macintosh)');
    mock.api.splashGet = vi.fn(async () => st());
    render(Page);
    expect(await screen.findByRole('status')).toHaveClass('sq');
    ua.mockRestore();
  });
});
