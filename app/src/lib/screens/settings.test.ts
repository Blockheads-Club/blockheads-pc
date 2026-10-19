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
import { setLang } from '../i18n.svelte';
import { fold, PAGES, searchSettings, SETTINGS } from '../settings-index';
import Settings from './Settings.svelte';
import SettingsAudio from './SettingsAudio.svelte';
import SettingsGame from './SettingsGame.svelte';
import SettingsGraphics from './SettingsGraphics.svelte';
import SettingsLanguage from './SettingsLanguage.svelte';

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
    render(SettingsGame);
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
    render(SettingsGame);
    expect(screen.getByText('TheBlockheads.apk')).toBeVisible();
  });

  it('labels an unknown build and an error', () => {
    store.apk = makeReport({ status: 'unknown', version: null });
    const { unmount } = render(SettingsGame);
    expect(screen.getByText('Unknown build · experimental')).toBeVisible();
    unmount();
    store.apk = makeReport({ status: 'error', version: null, problem: 'unreadable' });
    const again = render(SettingsGame);
    expect(screen.getByText('The file can’t be opened')).toBeVisible();
    again.unmount();
    store.apk = makeReport({ status: 'error', version: null, problem: null });
    const third = render(SettingsGame);
    expect(screen.getByText('The file is damaged')).toBeVisible();
    third.unmount();
    store.apk = null;
    store.config = null;
    render(SettingsGame);
    expect(screen.getByText('The Blockheads (unknown build)')).toBeVisible();
  });

  it('saves graphics, audio, language and theme right away', async () => {
    render(SettingsGraphics);
    await fireEvent.click(screen.getByRole('radio', { name: 'Fullscreen' }));
    await waitFor(() => expect(mock.api.saveConfig).toHaveBeenLastCalledWith(expect.objectContaining({ window: expect.objectContaining({ fullscreen: true }) })));
    expect(screen.getByText('Changes apply the next time the game starts.')).toBeVisible();
    cleanup();
    render(SettingsAudio);
    await fireEvent.click(screen.getByRole('switch', { name: 'Game sound' }));
    await waitFor(() => expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ game_sound: false })));
    expect(screen.getByRole('switch', { name: 'Mute when Blockheads PC is in the background' })).toBeDisabled();
    cleanup();
    render(SettingsLanguage);
    await fireEvent.change(screen.getByLabelText('Launcher language'), { target: { value: 'tr' } });
    await waitFor(() => expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ language: 'tr' })));
    await fireEvent.click(screen.getByRole('radio', { name: 'Koyu' }));
    await waitFor(() => expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ theme: 'dark' })));
  });
});

describe('Settings pages', () => {
  const nav = () => screen.getByRole('navigation', { name: 'Settings sections' });
  const item = (name: string) => within(nav()).getByRole('button', { name: new RegExp(`^${name}`) });

  it('shows one page per sidebar item, only the selected one', async () => {
    render(Settings);
    expect(item('Game & data')).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('heading', { name: 'Game file' })).toBeVisible();
    const pages: [string, string, string][] = [
      ['Window & graphics', 'gfx', 'Window & graphics'],
      ['Controls', 'controls', 'Controls'],
      ['Audio', 'audio', 'Audio'],
      ['Language & theme', 'lang', 'Language & theme'],
      ['Updates & about', 'about', 'Updates'],
      ['Game & data', 'game', 'Game file']
    ];
    const main = document.getElementById('main')!;
    for (const [label, id, heading] of pages) {
      main.scrollTop = 50;
      await fireEvent.click(item(label));
      expect(store.section).toBe(id);
      await waitFor(() => expect(item(label)).toHaveAttribute('aria-current', 'page'));
      expect(screen.getByRole('heading', { name: heading, level: heading === 'Controls' ? 1 : 2 })).toBeVisible();
      expect(within(nav()).getAllByRole('button').filter((b) => b.getAttribute('aria-current'))).toHaveLength(1);
      expect(main).toHaveFocus();
      expect(main.scrollTop).toBe(0);
    }
    expect(screen.queryByRole('heading', { name: 'Audio' })).toBeNull();
    await click('Back to home');
    expect(store.screen).toBe('home');
  });

  it('opens on the page that was shown last', () => {
    store.section = 'audio';
    render(Settings);
    expect(item('Audio')).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('switch', { name: 'Game sound' })).toBeVisible();
  });

  it('gives every row on every page a stable id that the search index knows', async () => {
    store.config = makeConfig({ window: { ...makeConfig().window, show_fps: true } });
    const ids = new Set(SETTINGS.map((s) => s.id));
    const seen = new Set<string>();
    for (const page of PAGES) {
      store.section = page.id;
      const { unmount } = render(Settings);
      for (const row of document.querySelectorAll<HTMLElement>('.row, [role="row"]')) {
        expect(row.id, row.textContent!).toMatch(/^set-/);
        seen.add(row.id);
      }
      for (const s of SETTINGS.filter((x) => x.page === page.id)) expect(document.getElementById(s.id), s.id).not.toBeNull();
      unmount();
    }
    // Every indexed setting is on its page; the only unindexed row is the long legal text under the short one.
    expect([...seen].filter((id) => !ids.has(id))).toEqual(['set-legal-full']);
  });
});

describe('Settings search', () => {
  const box = () => screen.getByRole('searchbox', { name: 'Search settings' });
  const type = (q: string) => fireEvent.input(box(), { target: { value: q } });
  const results = () => screen.queryByRole('region', { name: 'Search results' });
  const nav = () => screen.getByRole('navigation', { name: 'Settings sections' });
  const titles = () => [...document.querySelectorAll('.res .rt')].map((e) => e.textContent);

  it('folds case, accents and the Turkish i', () => {
    expect(fold('İSTANBUL Işık ŞĞÇÖÜ')).toBe('istanbul isik sgcou');
    expect(searchSettings('   ')).toEqual([]);
  });

  it('finds settings on every page by title, hint and keywords, grouped by page', async () => {
    render(Settings);
    expect(box()).toHaveAttribute('placeholder', 'Search settings');
    await type('fps');
    expect(results()).not.toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent('3 settings found');
    expect(titles()).toEqual(['FPS limit', 'Show FPS', 'Counter position']);
    expect(within(results()!).getByRole('heading', { name: 'Window & graphics' })).toBeVisible();
    expect(within(nav()).getByLabelText('3 matches')).toBeVisible();
    expect(within(nav()).getByRole('button', { name: /^Audio/ })).toHaveClass('dim');
    expect(within(nav()).getByRole('button', { name: /^Window/ })).not.toHaveClass('dim');
    // Keywords in either language, any case, with or without Turkish letters.
    await type('TAM EKRAN');
    expect(titles()).toEqual(['Display mode', 'Fullscreen']);
    expect(within(results()!).getAllByRole('heading').map((h) => h.textContent)).toEqual(['Window & graphics', 'Controls']);
    // "hız" is speed, and also frame rate ("kare hızı").
    await type('hiz');
    expect(titles()).toEqual(['FPS limit', 'Show FPS', 'Game speed']);
    // Title matches first within a page.
    await type('game speed');
    expect(titles()).toEqual(['Game speed', 'FPS limit']);
    await type('volume');
    expect(titles()).toEqual(['Game sound']);
    await type('dil');
    expect(titles()).toEqual(['Launcher language']);
    await type('vsync');
    expect(titles()).toEqual(['VSync']);
    // Hints and page names match too, and every word must match.
    await type('tearing smoother');
    expect(titles()).toEqual(['VSync']);
    await type('audio');
    expect(titles()).toEqual(['Game sound', 'Mute when Blockheads PC is in the background']);
    expect(within(nav()).getByLabelText('2 matches')).toBeVisible();
    await type('zzzz');
    expect(screen.getByText('No settings match “zzzz”')).toBeVisible();
    expect(screen.getByRole('status')).toHaveTextContent('0 settings found');
    await click('Clear search');
    expect(results()).toBeNull();
    expect(box()).toHaveFocus();
    expect(screen.getByRole('heading', { name: 'Game file' })).toBeVisible();
  });

  it('searches in Turkish', async () => {
    setLang('tr');
    render(Settings);
    const box = () => screen.getByRole('searchbox', { name: 'Ayarlarda ara' });
    const type = (q: string) => fireEvent.input(box(), { target: { value: q } });
    expect(box()).toHaveAttribute('placeholder', 'Ayarlarda ara');
    await type('ses');
    expect(titles()).toContain('Oyun sesi');
    await type('ISIK');
    expect(screen.getByText('“ISIK” ile eşleşen ayar yok')).toBeVisible();
    await type('kontrol');
    expect(titles()).toContain('Dokun ve sürükle');
  });

  it('opens a result: its page, scrolled to and highlighted, its control focused', async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    render(Settings);
    await type('show fps');
    await fireEvent.click(screen.getByRole('button', { name: /^Show FPS/ }));
    expect(store.section).toBe('gfx');
    expect(results()).toBeNull();
    expect(box()).toHaveValue('');
    const row = document.getElementById('set-gfx-show-fps')!;
    expect(scroll).toHaveBeenCalledWith({ block: 'center', behavior: 'smooth' });
    expect(row).toHaveClass('flash');
    expect(screen.getByRole('switch', { name: 'Show FPS' })).toHaveFocus();
    await waitFor(() => expect(row).not.toHaveClass('flash'), { timeout: 3000 });
  });

  it('falls back to a visible row, focuses rows without a control, and respects reduced motion', async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    const media = window.matchMedia;
    window.matchMedia = ((q: string) => ({ ...media(q), matches: true })) as typeof window.matchMedia;
    render(Settings);
    // The counter position is hidden while the counter is off: its switch is shown instead.
    await type('counter position');
    await fireEvent.click(screen.getByRole('button', { name: /^Counter position/ }));
    expect(screen.getByRole('switch', { name: 'Show FPS' })).toHaveFocus();
    expect(scroll).toHaveBeenLastCalledWith({ block: 'center', behavior: 'auto' });
    await type('legal');
    await fireEvent.click(screen.getByRole('button', { name: /^Legal notice/ }));
    const legal = document.getElementById('set-legal')!;
    expect(legal).toHaveFocus();
    expect(legal).toHaveAttribute('tabindex', '-1');
    // The theme's selected option is the one that gets focus.
    await type('theme');
    await fireEvent.click(screen.getByRole('button', { name: /^Theme/ }));
    expect(screen.getByRole('radio', { name: 'System' })).toHaveFocus();
    window.matchMedia = media;
  });

  it('focuses the page when the setting is not shown at all', async () => {
    store.config = null;
    render(Settings);
    await type('vsync');
    await fireEvent.keyDown(box(), { key: 'Enter' });
    await waitFor(() => expect(document.getElementById('main')).toHaveFocus());
    expect(store.section).toBe('gfx');
  });

  it('is driven by the keyboard', async () => {
    render(Settings);
    // Ctrl+F, Cmd+F and "/" focus the field.
    await fireEvent.keyDown(document.body, { key: 'f', ctrlKey: true });
    expect(box()).toHaveFocus();
    box().blur();
    await fireEvent.keyDown(document.body, { key: 'F', metaKey: true });
    expect(box()).toHaveFocus();
    box().blur();
    await fireEvent.keyDown(document.body, { key: '/' });
    expect(box()).toHaveFocus();
    box().blur();
    // Not with other modifiers, not while typing elsewhere, not over a dialog, not when already handled.
    for (const ev of [{ key: 'f', ctrlKey: true, altKey: true }, { key: 'f' }, { key: '/', ctrlKey: true }, { key: '/', metaKey: true }, { key: '/', altKey: true }]) {
      await fireEvent.keyDown(document.body, ev);
      expect(box()).not.toHaveFocus();
    }
    await fireEvent.click(within(nav()).getByRole('button', { name: /^Language/ }));
    const select = screen.getByLabelText('Launcher language');
    select.focus();
    await fireEvent.keyDown(select, { key: '/' });
    expect(select).toHaveFocus();
    const dlg = document.body.appendChild(Object.assign(document.createElement('div'), { role: 'dialog' }));
    dlg.setAttribute('role', 'dialog');
    await fireEvent.keyDown(document.body, { key: 'f', ctrlKey: true });
    expect(box()).not.toHaveFocus();
    dlg.remove();
    const handled = new KeyboardEvent('keydown', { key: '/', bubbles: true, cancelable: true });
    handled.preventDefault();
    document.body.dispatchEvent(handled);
    expect(box()).not.toHaveFocus();

    // Arrows walk the results, Esc clears.
    box().focus();
    await type('fps');
    await fireEvent.keyDown(box(), { key: 'x' });
    await fireEvent.keyDown(box(), { key: 'ArrowDown' });
    const res = [...document.querySelectorAll<HTMLElement>('button.res')];
    expect(res[0]).toHaveFocus();
    await fireEvent.keyDown(res[0], { key: 'ArrowDown' });
    expect(res[1]).toHaveFocus();
    await fireEvent.keyDown(res[1], { key: 'ArrowDown' });
    await fireEvent.keyDown(res[2], { key: 'ArrowDown' });
    expect(res[2]).toHaveFocus();
    await fireEvent.keyDown(res[2], { key: 'ArrowUp' });
    expect(res[1]).toHaveFocus();
    await fireEvent.keyDown(res[1], { key: 'Tab' });
    await fireEvent.keyDown(res[1], { key: 'ArrowUp' });
    await fireEvent.keyDown(res[0], { key: 'ArrowUp' });
    expect(box()).toHaveFocus();
    await fireEvent.keyDown(res[0], { key: 'Escape' });
    expect(results()).toBeNull();
    expect(box()).toHaveFocus();
    await type('fps');
    await fireEvent.keyDown(box(), { key: 'Escape' });
    expect(box()).toHaveValue('');
    await fireEvent.keyDown(box(), { key: 'Escape' });
    // No results: arrows and Enter do nothing.
    await type('zzzz');
    await fireEvent.keyDown(box(), { key: 'ArrowDown' });
    await fireEvent.keyDown(box(), { key: 'Enter' });
    expect(box()).toHaveFocus();
    // Enter opens the first result.
    await type('game speed');
    await fireEvent.keyDown(box(), { key: 'Enter' });
    await waitFor(() => expect(screen.getByLabelText('Game speed')).toHaveFocus());
  });

  it('a sidebar item ends the search and opens its page', async () => {
    render(Settings);
    await type('fps');
    await fireEvent.click(within(nav()).getByRole('button', { name: /^Controls/ }));
    expect(results()).toBeNull();
    expect(store.section).toBe('controls');
    expect(screen.getByRole('table')).toBeVisible();
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
