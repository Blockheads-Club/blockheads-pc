import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('../api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import { makeApi, makeConfig, makeReport, makeWin } from '../mock-api';
import { store } from '../store.svelte';
import { resetStore } from '../test-utils';
import { toasts } from '../ui/toast.svelte';
import SettingsAbout from './SettingsAbout.svelte';
import SettingsControls from './SettingsControls.svelte';
import SettingsGeneral from './SettingsGeneral.svelte';

const click = (name: string | RegExp) => fireEvent.click(screen.getByRole('button', { name }));
const clipboard = vi.fn(async () => {});

beforeEach(() => {
  mock.api = makeApi();
  mock.win = makeWin().win;
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: clipboard }, configurable: true });
  clipboard.mockClear();
  resetStore({ screen: 'settings' });
});
afterEach(() => cleanup());

describe('Settings > game, graphics, audio, language', () => {
  it('shows the game file and data folder and acts on them', async () => {
    mock.api.dataSize = vi.fn(async () => 222_000_000);
    render(SettingsGeneral);
    expect(screen.getByText('The Blockheads 1.7.6')).toBeVisible();
    expect(screen.getByText(/61.3 MB · TheBlockheads.apk/)).toBeVisible();
    expect(screen.getByText('Supported')).toBeVisible();
    await click('Change…');
    await waitFor(() => expect(mock.api.useApk).toHaveBeenCalled());
    await click('Check again');
    expect(mock.api.checkApk).toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('switch', { name: 'Allow unknown builds' }));
    await waitFor(() => expect(mock.api.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ force: true })));
    expect(await screen.findByText(/\/data · 211.7 MB/)).toBeVisible();
    await click('Open');
    expect(mock.api.openPath).toHaveBeenCalledWith('/data');
    await click('Move…');
    await waitFor(() => expect(mock.api.moveDataDir).toHaveBeenCalledWith('/dl/saves'));
  });

  it('shows the file name without a check result', () => {
    store.apk = null;
    render(SettingsGeneral);
    expect(screen.getByText('TheBlockheads.apk')).toBeVisible();
  });

  it('labels an unknown build and an error', () => {
    store.apk = makeReport({ status: 'unknown', version: null });
    const { unmount } = render(SettingsGeneral);
    expect(screen.getByText('Unknown build · experimental')).toBeVisible();
    unmount();
    store.apk = makeReport({ status: 'error', version: null, problem: 'unreadable' });
    const again = render(SettingsGeneral);
    expect(screen.getByText('The file can’t be opened')).toBeVisible();
    again.unmount();
    store.apk = makeReport({ status: 'error', version: null, problem: null });
    const third = render(SettingsGeneral);
    expect(screen.getByText('The file is damaged')).toBeVisible();
    third.unmount();
    store.apk = null;
    store.config = null;
    render(SettingsGeneral);
    expect(screen.getByText('The Blockheads (unknown build)')).toBeVisible();
  });

  it('saves graphics, audio, language and theme right away', async () => {
    render(SettingsGeneral);
    await fireEvent.click(screen.getByRole('radio', { name: 'Fullscreen' }));
    await waitFor(() => expect(mock.api.saveConfig).toHaveBeenLastCalledWith(expect.objectContaining({ window: expect.objectContaining({ fullscreen: true }) })));
    await fireEvent.click(screen.getByRole('switch', { name: 'Game sound' }));
    await waitFor(() => expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ game_sound: false })));
    expect(screen.getByRole('switch', { name: 'Mute when Blockheads PC is in the background' })).toBeDisabled();
    await fireEvent.change(screen.getByLabelText('Launcher language'), { target: { value: 'tr' } });
    await waitFor(() => expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ language: 'tr' })));
    await fireEvent.click(screen.getByRole('radio', { name: 'Koyu' }));
    await waitFor(() => expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ theme: 'dark' })));
  });

  it('navigates between sections', async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    render(SettingsGeneral);
    const nav = screen.getByRole('navigation', { name: 'Settings sections' });
    expect(within(nav).getByRole('button', { name: 'Game & data' })).toHaveAttribute('aria-current', 'page');
    await fireEvent.click(within(nav).getByRole('button', { name: 'Window & graphics' }));
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    expect(within(nav).getByRole('button', { name: 'Window & graphics' })).toHaveAttribute('aria-current', 'page');
    await fireEvent.click(within(nav).getByRole('button', { name: 'Audio' }));
    await fireEvent.click(within(nav).getByRole('button', { name: 'Language & theme' }));
    await fireEvent.click(within(nav).getByRole('button', { name: 'Controls' }));
    expect(store.section).toBe('controls');
    await waitFor(() => expect(within(nav).getByRole('button', { name: 'Controls' })).toHaveAttribute('aria-current', 'page'));
    store.section = 'general';
    await waitFor(() => expect(within(nav).getByRole('button', { name: 'Game & data' })).toHaveAttribute('aria-current', 'page'));
    await fireEvent.click(within(nav).getByRole('button', { name: 'Updates & about' }));
    expect(store.section).toBe('about');
    await waitFor(() => expect(within(nav).getByRole('button', { name: 'Updates & about' })).toHaveAttribute('aria-current', 'page'));
    store.section = 'general';
    await waitFor(() => expect(within(nav).getByRole('button', { name: 'Game & data' })).toHaveAttribute('aria-current', 'page'));
    await click('Back to home');
    expect(store.screen).toBe('home');
  });

  it('keeps the section when general is re-selected from a general anchor', async () => {
    store.section = 'general';
    render(SettingsGeneral);
    await fireEvent.click(within(screen.getByRole('navigation', { name: 'Settings sections' })).getByRole('button', { name: 'Game & data' }));
    expect(store.section).toBe('general');
  });
});

describe('Settings > controls', () => {
  beforeEach(() => (store.section = 'controls'));

  it('marks changes and resets one or all', async () => {
    store.config = makeConfig({ keys: { ...makeConfig().keys, back: 'Backspace' } });
    render(SettingsControls);
    expect(screen.getByRole('img', { name: 'changed from default' })).toBeVisible();
    await click('Reset Back to default');
    await waitFor(() => expect(store.config?.keys.back).toBe('Esc'));
    await click('Reset all to defaults…');
    expect(screen.getByRole('dialog', { name: 'Reset all controls?' })).toBeVisible();
    await fireEvent.keyDown(screen.getByRole('dialog').parentElement!, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    await click('Reset all to defaults…');
    await click('Cancel');
    expect(screen.queryByRole('dialog')).toBeNull();
    await click('Reset all to defaults…');
    await click('Reset controls');
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('explains a clash and helps fix it', async () => {
    store.config = makeConfig({ keys: { ...makeConfig().keys, fullscreen: 'Esc' } });
    store.undoKeys = makeConfig().keys;
    render(SettingsControls);
    expect(screen.getByRole('alert')).toHaveTextContent('Esc is used twice');
    expect(screen.getByRole('alert')).toHaveTextContent('“Back” and “Fullscreen” use the same key');
    await click('Change Back');
    expect(await screen.findByText('Press a key or mouse action')).toBeVisible();
    await fireEvent.keyDown(window, { key: 'Escape' });
    await click('Undo last change');
    await waitFor(() => expect(store.config?.keys.fullscreen).toBe('F11'));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('shows nothing to edit without a configuration', () => {
    store.config = null;
    render(SettingsControls);
    expect(screen.queryByRole('table')).toBeNull();
  });

  it('has no undo without a previous change', () => {
    store.config = makeConfig({ keys: { ...makeConfig().keys, fullscreen: 'Esc' } });
    render(SettingsControls);
    expect(screen.queryByRole('button', { name: 'Undo last change' })).toBeNull();
  });
});

describe('Settings > updates and about', () => {
  beforeEach(() => (store.section = 'about'));
  const info = { version: '0.2.0', size_mb: 43, notes: [] };

  it('copes without backend info', () => {
    store.info = null;
    render(SettingsAbout);
    expect(screen.getByText(/a Blockheads Club project/)).toBeVisible();
    expect(screen.getByText('Update checks are not available in this build')).toBeVisible();
    cleanup();
    store.update = { stage: 'idle', info: null, progress: null, error: '', checked: false };
    store.info = { version: '1', os: 'linux', arch: 'x', config_dir: '', updates_enabled: true, native_controls: false };
    render(SettingsAbout);
    expect(screen.getByText('Blockheads PC 1')).toBeVisible();
  });

  it('says updates are off in this build', () => {
    render(SettingsAbout);
    expect(screen.getByText('Update checks are not available in this build')).toBeVisible();
    expect(screen.getByRole('switch', { name: 'Check automatically once a week' })).toBeDisabled();
    expect(screen.getByText('Version 9.9.9 · linux x86_64 · a Blockheads Club project')).toBeVisible();
  });

  it('walks through the update states', async () => {
    store.info = { ...store.info!, updates_enabled: true };
    const states: [any, string][] = [
      ['idle', 'Not checked yet'],
      ['checking', 'Checking for updates…'],
      ['uptodate', 'You have the latest version'],
      ['available', 'Version 0.2.0 is available'],
      ['downloading', 'Version 0.2.0 is available'],
      ['error', 'Couldn’t check for updates']
    ];
    for (const [stage, text] of states) {
      store.update = { stage, info, progress: null, error: '', checked: true };
      const { unmount } = render(SettingsAbout);
      expect(screen.getByText(text)).toBeVisible();
      unmount();
    }
    store.update = { stage: 'idle', info: null, progress: null, error: '', checked: false };
    render(SettingsAbout);
    await click('Check for updates');
    await waitFor(() => expect(store.update.stage).toBe('available'));
    await fireEvent.click(await screen.findByRole('button', { name: 'Update…' }));
    expect(store.dialog).toBe('update');
    await fireEvent.click(screen.getByRole('switch', { name: 'Check automatically once a week' }));
    expect(mock.api.savePrefs).toHaveBeenCalledWith(expect.objectContaining({ auto_update_check: false }));
    cleanup();
    for (const stage of ['uptodate', 'error'] as const) {
      store.update = { stage, info, progress: null, error: '', checked: true };
      render(SettingsAbout);
      await click(stage === 'uptodate' ? 'Check again' : 'Try again');
      cleanup();
    }
  });

  it('copies system info and links to GitHub', async () => {
    render(SettingsAbout);
    await click('Copy system info');
    await waitFor(() => expect(toasts.items.at(-1)?.message).toBe('System info copied'));
    clipboard.mockRejectedValueOnce(new Error('no'));
    await click('Copy system info');
    await waitFor(() => expect(toasts.items.at(-1)?.message).toBe('Could not copy to the clipboard'));
    await click('GitHub');
    expect(mock.api.openUrl).toHaveBeenCalled();
    expect(screen.getByText(/GPL-3.0-only/, { selector: 'span' })).toBeVisible();
  });

  it('shows the third-party notices', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ text: async () => 'NOTICES TEXT' })));
    render(SettingsAbout);
    await click('Show all licences');
    expect(await screen.findByText('NOTICES TEXT')).toBeVisible();
    await fireEvent.keyDown(screen.getByRole('dialog').parentElement!, { key: 'Escape' });
    expect(screen.queryByText('NOTICES TEXT')).toBeNull();
    await click('Show all licences');
    await fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[1]);
    vi.stubGlobal('fetch', vi.fn(async () => { throw new Error('offline'); }));
    await click('Show all licences');
    expect(await screen.findByText('The notices file could not be loaded.')).toBeVisible();
    vi.unstubAllGlobals();
  });
});
