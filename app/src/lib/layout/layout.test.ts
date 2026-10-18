import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ win: {} as Record<string, any> }));
vi.mock('../api', () => ({ api: {}, win: new Proxy({}, { get: (_t, k: string) => mock.win[k] }) }));

import { makeWin } from '../mock-api';
import TitleBar from './TitleBar.svelte';

let hooks: ReturnType<typeof makeWin>['hooks'];
beforeEach(() => {
  const w = makeWin();
  mock.win = w.win;
  hooks = w.hooks;
});
afterEach(() => cleanup());

describe('TitleBar', () => {
  it('draws Windows controls and drives the window', async () => {
    render(TitleBar, { os: 'windows', ctx: 'Home' });
    expect(screen.getByRole('banner', { name: 'Window title bar' })).toBeVisible();
    expect(screen.getByText('· Home')).toBeVisible();
    await fireEvent.click(screen.getByRole('button', { name: 'Minimize window' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Maximize window' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Close window' }));
    expect(mock.win.minimize).toHaveBeenCalled();
    expect(mock.win.toggleMaximize).toHaveBeenCalled();
    expect(mock.win.close).toHaveBeenCalled();
  });

  it('follows maximize and restore', async () => {
    render(TitleBar, { os: 'windows' });
    await waitFor(() => expect(mock.win.isMaximized).toHaveBeenCalled());
    expect(screen.getByRole('button', { name: 'Maximize window' })).toBeVisible();
    mock.win.isMaximized = vi.fn(async () => true);
    hooks.resize?.();
    expect(await screen.findByRole('button', { name: 'Restore window' })).toBeVisible();
    expect(screen.queryByText(/^·/)).toBeNull();
  });

  it('can be forced into the maximized look', () => {
    render(TitleBar, { os: 'windows', maximized: true });
    expect(screen.getByRole('button', { name: 'Restore window' })).toBeVisible();
  });

  it('draws round Linux buttons', async () => {
    render(TitleBar, { os: 'linux', ctx: 'Settings', inactive: true, onStage: true });
    await fireEvent.click(screen.getByRole('button', { name: 'Minimize window' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Maximize window' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Close window' }));
    expect(mock.win.close).toHaveBeenCalled();
  });

  it('draws traffic lights on macOS unless the OS provides them', async () => {
    const { unmount } = render(TitleBar, { os: 'macos', ctx: 'Home', inactive: true });
    await fireEvent.click(screen.getByRole('button', { name: 'Close window' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Minimize window' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Maximize window' }));
    expect(mock.win.toggleMaximize).toHaveBeenCalled();
    unmount();
    render(TitleBar, { os: 'macos', native: true });
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('works when the window API is unavailable', async () => {
    mock.win.isMaximized = vi.fn(async () => { throw new Error('no window'); });
    mock.win.onResized = vi.fn(async () => { throw new Error('no window'); });
    render(TitleBar, { os: 'windows' });
    await waitFor(() => expect(mock.win.onResized).toHaveBeenCalled());
    expect(screen.getByRole('button', { name: 'Maximize window' })).toBeVisible();
  });
});
