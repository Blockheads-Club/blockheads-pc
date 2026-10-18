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
import SetupApk from './SetupApk.svelte';
import SetupControls from './SetupControls.svelte';
import SetupGraphics from './SetupGraphics.svelte';
import SetupImport from './SetupImport.svelte';
import SetupReady from './SetupReady.svelte';
import SetupWelcome from './SetupWelcome.svelte';
import Splash from './Splash.svelte';
import type { SplashState } from '$lib/types';

let hooks: ReturnType<typeof makeWin>['hooks'];
const click = (name: string | RegExp) => fireEvent.click(screen.getByRole('button', { name }));

beforeEach(() => {
  mock.api = makeApi();
  const w = makeWin();
  mock.win = w.win;
  hooks = w.hooks;
  resetStore({ screen: 'setup', draft: makeConfig({ apk: '' }), config: null, apk: null });
});
afterEach(() => cleanup());

const splash = (over: Partial<SplashState> = {}): SplashState => ({
  boot: 'loading', error: '', first_run: false, version: '9.9.9', config_dir: '/cfg', lang: 'en', ...over
});

describe('Splash', () => {
  const fns = () => ({ onretry: vi.fn(), onopenlog: vi.fn(), onquit: vi.fn() });

  it('shows progress in both variants', () => {
    const { unmount } = render(Splash, { state: splash(), ...fns() });
    expect(screen.getByRole('status', { name: 'Blockheads PC is starting' })).toBeVisible();
    expect(screen.getByText('Checking your game file…')).toBeVisible();
    unmount();
    render(Splash, { state: splash({ boot: 'slow', first_run: true }), square: true, ...fns() });
    expect(screen.getByRole('status', { name: 'Blockheads PC setup is starting' })).toBeVisible();
    expect(screen.getByText('This is taking longer than usual. Still working…')).toBeVisible();
    cleanup();
    render(Splash, { state: splash({ boot: 'slow' }), ...fns() });
    expect(screen.getByText(/Taking longer than usual/)).toBeVisible();
    cleanup();
    render(Splash, { state: splash({ first_run: true }), ...fns() });
    expect(screen.getByText('Preparing setup…')).toBeVisible();
  });

  it('offers retry, the config folder and quit after a failed start', async () => {
    const f = fns();
    render(Splash, { state: splash({ boot: 'error', error: 'i/o error: denied' }), ...f });
    expect(screen.getByRole('alert')).toHaveTextContent('couldn’t open its settings');
    expect(screen.getByText('i/o error: denied')).toBeVisible();
    await click('Open folder');
    expect(f.onopenlog).toHaveBeenCalled();
    await click('Quit');
    expect(f.onquit).toHaveBeenCalled();
    await click('Try again');
    expect(f.onretry).toHaveBeenCalled();
    cleanup();
    render(Splash, { state: splash({ boot: 'error', first_run: true }), ...fns() });
    expect(screen.getByRole('alert')).toHaveTextContent('Setup couldn’t create its data folder');
  });
});

describe('Welcome', () => {
  it('needs the legal checkbox, toggles the theme and switches language', async () => {
    render(SetupWelcome);
    expect(screen.getByRole('heading', { name: 'Play The Blockheads on your computer' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    await fireEvent.click(screen.getByLabelText(/I own the game/));
    expect(store.legalOk).toBe(true);
    expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled();
    await click('Dark mode');
    expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ theme: 'dark' }));
    expect(screen.getByRole('button', { name: 'Light mode' })).toHaveAttribute('aria-pressed', 'true');
    await click('Light mode');
    expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ theme: 'light' }));
    await fireEvent.change(screen.getByLabelText('Language'), { target: { value: 'tr' } });
    expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ language: 'tr' }));
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('The Blockheads’i bilgisayarında oyna');
    await fireEvent.click(screen.getByText('Yasal bildirimin tamamını oku'));
  });
});

describe('Game file step', () => {
  beforeEach(() => (store.step = 2));

  it('starts with a drop zone and a link to the guide', async () => {
    render(SetupApk);
    expect(screen.getByRole('group', { name: 'Drop zone for the game APK' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    const zone = screen.getByRole('group', { name: 'Drop zone for the game APK' });
    await fireEvent.dragEnter(zone);
    expect(zone).toHaveClass('over');
    await fireEvent.dragOver(zone);
    await fireEvent.dragLeave(zone);
    await fireEvent.dragEnter(zone);
    await fireEvent.drop(zone);
    expect(zone).not.toHaveClass('over');
    await fireEvent.click(screen.getByRole('link', { name: /Our guide shows/ }));
    expect(mock.api.openUrl).toHaveBeenCalled();
    await click('Browse…');
    await waitFor(() => expect(screen.getByText('The Blockheads 1.7.6')).toBeVisible());
    expect(screen.getByText('Supported')).toBeVisible();
    expect(screen.getByText('61.3 MB')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled();
  });

  it('takes a dropped file and ignores empty drops', async () => {
    render(SetupApk);
    await waitFor(() => expect(hooks.drop).toBeDefined());
    hooks.drop?.([]);
    expect(mock.api.checkApk).not.toHaveBeenCalled();
    hooks.drop?.(['/dl/dropped.apk']);
    await waitFor(() => expect(store.draft?.apk).toBe('/dl/dropped.apk'));
  });

  it('survives a window without drop support', async () => {
    mock.win.onFileDrop = vi.fn(async () => { throw new Error('no webview'); });
    render(SetupApk);
    await waitFor(() => expect(mock.win.onFileDrop).toHaveBeenCalled());
    expect(screen.getByRole('group', { name: 'Drop zone for the game APK' })).toBeVisible();
  });

  it('shows the check in progress', () => {
    store.apkChecking = true;
    store.apk = makeReport({ path: '/dl/Some.apk' });
    render(SetupApk);
    expect(screen.getByRole('status')).toHaveTextContent('Checking the file');
    expect(screen.getByText('Some.apk')).toBeVisible();
    cleanup();
    store.apk = null;
    render(SetupApk);
    expect(screen.getByRole('progressbar', { name: 'Checking the game file' })).toBeVisible();
  });

  it('shows a supported and an unknown build, and the private copy switch', async () => {
    store.apk = makeReport();
    render(SetupApk);
    await fireEvent.click(screen.getByRole('switch', { name: 'Keep a private copy in the data folder' }));
    expect(mock.api.savePrefs).toHaveBeenCalledWith(expect.objectContaining({ keep_private_copy: false }));
    await click('Change');
    expect(mock.api.pickFile).toHaveBeenCalled();
    cleanup();
    store.apk = makeReport({ status: 'unknown', version: null });
    render(SetupApk);
    expect(screen.getByText('The Blockheads (unknown build)')).toBeVisible();
    expect(screen.getByText('Unknown build · experimental')).toBeVisible();
  });

  for (const problem of ['not_blockheads', 'unsupported', 'damaged', 'unreadable'] as const) {
    it(`explains ${problem} and offers a fix`, async () => {
      store.apk = makeReport({ status: 'error', version: null, problem, detail: 'technical detail', path: '/dl/f.apk' });
      render(SetupApk);
      expect(screen.getByRole('alert')).toBeVisible();
      expect(screen.getByText(/technical detail/)).toBeVisible();
      expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
      await fireEvent.click(screen.getAllByRole('button').find((b) => b.textContent?.match(/APK|versions|permissions/))!);
      expect(mock.api.openUrl).toHaveBeenCalled();
      await click('Choose another file');
      expect(mock.api.pickFile).toHaveBeenCalled();
    });
  }

  it('shows an error without a cause or detail, and the switch without a draft', () => {
    store.apk = makeReport({ status: 'error', version: null, problem: null, detail: '' });
    render(SetupApk);
    expect(screen.getByText('The file is damaged')).toBeVisible();
    cleanup();
    store.apk = null;
    store.draft = null;
    render(SetupApk);
    expect(screen.getByRole('switch', { name: 'Allow unknown builds' })).not.toBeChecked();
  });

  it('lets the user allow unknown builds', async () => {
    store.apk = makeReport({ status: 'error', version: null, problem: 'unsupported' });
    render(SetupApk);
    const sw = screen.getByRole('switch', { name: 'Allow unknown builds' });
    expect(screen.queryByText('Experimental.')).toBeNull();
    await fireEvent.click(sw);
    expect(store.draft?.force).toBe(true);
    expect(await screen.findByText('Experimental.')).toBeVisible();
  });
});

describe('Window and graphics step', () => {
  beforeEach(() => (store.step = 3));

  it('shows the detected display and edits the draft', async () => {
    render(SetupGraphics);
    await click('Next');
    expect(store.step).toBe(4);
    store.step = 3;
    expect(screen.getByText('Detected: 2560 × 1440 · scale 125 %')).toBeVisible();
    expect(screen.getByRole('img', { name: /interface size at 125 percent/ })).toBeVisible();
    const size = screen.getByLabelText('Window size') as HTMLSelectElement;
    expect([...size.options].map((o) => o.textContent)).toContain('1920 × 1080 (recommended)');
    expect([...size.options].map((o) => o.textContent)).toContain('2560 × 1440 (screen)');
    await fireEvent.change(size, { target: { value: '1920x1080' } });
    expect(store.draft?.window).toMatchObject({ width: 1920, height: 1080 });
    await fireEvent.click(screen.getByRole('radio', { name: 'Fullscreen' }));
    expect(store.draft?.window.fullscreen).toBe(true);
    await fireEvent.click(screen.getByRole('switch', { name: 'VSync' }));
    expect(store.draft?.window.vsync).toBe(false);
    const scale = screen.getByLabelText('Game UI scale');
    await fireEvent.change(scale, { target: { value: '150' } });
    expect(store.draft?.window.ui_scale).toBe(1.5);
    expect(screen.getByText('150 %')).toBeVisible();
    expect(scale).toHaveAttribute('aria-valuetext', '150 %');
  });

  it('renders nothing editable without a draft', () => {
    store.draft = null;
    render(SetupGraphics);
    expect(screen.queryByLabelText('Window size')).toBeNull();
    cleanup();
    render(SetupControls);
    expect(screen.queryByRole('table')).toBeNull();
  });

  it('works without a detected display and lists an unusual current size', () => {
    store.display = null;
    store.draft = makeConfig({ window: { width: 1111, height: 700, fullscreen: false, vsync: true, ui_scale: 1 } });
    render(SetupGraphics);
    expect(screen.queryByText(/Detected/)).toBeNull();
    expect([...(screen.getByLabelText('Window size') as HTMLSelectElement).options].map((o) => o.textContent)).toContain('1111 × 700');
  });

  it('does not repeat a display size that is already standard', () => {
    store.display = { width: 1920, height: 1080, scale: 1 };
    render(SetupGraphics);
    const labels = [...(screen.getByLabelText('Window size') as HTMLSelectElement).options].map((o) => o.textContent);
    expect(labels.filter((l) => l?.startsWith('1920'))).toHaveLength(1);
  });
});

describe('Controls step', () => {
  beforeEach(() => (store.step = 4));

  it('rebinds with a key, a mouse button and the wheel', async () => {
    render(SetupControls);
    expect(screen.getByRole('table', { name: 'Control bindings' })).toBeVisible();
    await click('Change binding for Back');
    expect(screen.getByRole('status')).toHaveTextContent('Press a key or mouse action');
    await fireEvent.keyDown(window, { key: 'Backspace' });
    await waitFor(() => expect(store.draft?.keys.back).toBe('Backspace'));
    expect(screen.getByText('Backspace')).toBeVisible();

    await click('Change binding for Pan');
    await fireEvent.mouseDown(document.body, { button: 1 });
    await waitFor(() => expect(store.draft?.keys.pan).toBe('Middle'));

    await click('Change binding for Zoom');
    await fireEvent.wheel(document.body, { ctrlKey: true });
    await waitFor(() => expect(store.draft?.keys.zoom).toBe('Ctrl+Wheel'));
  });

  it('cancels with Esc, the Cancel button and ignores bare modifiers', async () => {
    render(SetupControls);
    await click('Change binding for Back');
    await fireEvent.keyDown(window, { key: 'Shift' });
    expect(screen.getByRole('status')).toBeVisible();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('status')).toBeNull();
    expect(store.draft?.keys.back).toBe('Esc');
    await click('Change binding for Back');
    await click('Cancel');
    expect(screen.queryByRole('status')).toBeNull();
    // Input while nothing is being captured passes through.
    await fireEvent.keyDown(window, { key: 'q' });
    await fireEvent.mouseDown(document.body);
    await fireEvent.wheel(document.body);
    expect(store.draft?.keys).toEqual(makeConfig().keys);
    // Ctrl+Esc is a binding, not a cancel.
    await click('Change binding for Back');
    await fireEvent.keyDown(window, { key: 'Escape', ctrlKey: true });
    await waitFor(() => expect(store.draft?.keys.back).toBe('Ctrl+Esc'));
  });

  it('shows a refused binding as an error and keeps the old one', async () => {
    mock.api.normalizeBinding = vi.fn(async () => { throw 'invalid binding: F13'; });
    render(SetupControls);
    await click('Change binding for Back');
    await fireEvent.keyDown(window, { key: 'F13' });
    await waitFor(() => expect(toasts.items[0]?.message).toBe('invalid binding: F13'));
    expect(store.draft?.keys.back).toBe('Esc');
  });

  it('marks clashing keys and resets everything', async () => {
    store.draft = makeConfig({ keys: { ...makeConfig().keys, back: 'F11' } });
    render(SetupControls);
    expect(screen.getAllByText('Same key as Fullscreen')[0]).toBeVisible();
    await click('Reset to defaults');
    await waitFor(() => expect(store.draft?.keys.back).toBe('Esc'));
  });
});

describe('Import step', () => {
  beforeEach(() => (store.step = 5));

  it('chooses a source and imports', async () => {
    render(SetupImport);
    expect(screen.getByRole('button', { name: 'Import' })).toBeDisabled();
    expect(screen.getByTestId('import-path')).toHaveTextContent('Nothing chosen yet');
    await click('Browse…');
    await waitFor(() => expect(screen.getByTestId('import-path')).toHaveTextContent('/dl/saves'));
    await fireEvent.click(screen.getByLabelText(/Replace it/));
    expect(store.importOverwrite).toBe(true);
    await fireEvent.click(screen.getByLabelText(/Skip it/));
    expect(store.importOverwrite).toBe(false);
    await fireEvent.click(screen.getByLabelText(/A zip file/));
    expect(store.importKind).toBe('zip');
    expect(store.importPath).toBe('');
    await fireEvent.click(screen.getByLabelText(/A folder/));
    await click('Browse…');
    await waitFor(() => expect(store.importPath).toBe('/dl/saves'));
    await click('Import');
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('3 files imported, 1 skipped.'));
    expect(screen.getByText('saves/a')).toBeVisible();
    expect(screen.getByText('1 file already existed')).toBeVisible();
    expect(screen.getByText('2 items not used on PC')).toBeVisible();
    await click('Import more');
    expect(store.importSummary).toBeNull();
    expect(screen.getByRole('button', { name: 'Skip' })).toBeVisible();
  });

  it('skips to the next step', async () => {
    render(SetupImport);
    await click('Skip');
    expect(store.step).toBe(6);
  });

  it('shows progress and then lets the user go on', async () => {
    store.importing = true;
    store.importPath = '/dl/saves';
    render(SetupImport);
    expect(screen.getByRole('progressbar', { name: 'Import progress' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Importing…' })).toBeDisabled();
    cleanup();
    store.importing = false;
    store.importSummary = { written: 1, skipped_existing: 0, ignored: 0, items: [{ name: 'x', files: 1, bytes: 10 }] };
    render(SetupImport);
    expect(screen.queryByText(/already existed/)).toBeNull();
    expect(screen.queryByText(/not used on PC/)).toBeNull();
    await click('Next');
    expect(store.step).toBe(6);
  });

  it('explains an empty source and an error', async () => {
    store.importPath = '/dl/empty';
    store.importSummary = { written: 0, skipped_existing: 0, ignored: 0, items: [] };
    render(SetupImport);
    expect(screen.getByRole('alert')).toHaveTextContent('No saves found here');
    await click('Choose another source');
    expect(store.importSummary).toBeNull();
    cleanup();
    store.importSummary = null;
    store.importError = 'bad zip';
    render(SetupImport);
    expect(screen.getByRole('alert')).toHaveTextContent('bad zip');
  });
});

describe('Ready step', () => {
  beforeEach(() => (store.step = 6));

  it('summarises the choices and starts the launcher', async () => {
    store.apk = makeReport();
    store.draft = makeConfig({ keys: { ...makeConfig().keys, back: 'Backspace' }, window: { width: 1600, height: 900, fullscreen: false, vsync: true, ui_scale: 1.25 } });
    store.importSummary = { written: 3, skipped_existing: 0, ignored: 0, items: [] };
    render(SetupReady);
    expect(screen.getByRole('heading', { name: 'You’re ready to play!' })).toBeVisible();
    expect(screen.getByText('Window · 1600 × 900 · VSync on · UI 125 %')).toBeVisible();
    expect(screen.getByText(/1 changed/)).toBeVisible();
    expect(screen.getByText(/Back → Backspace/)).toBeVisible();
    expect(screen.getByText('3 files imported from your phone')).toBeVisible();
    expect(screen.getByText('Private copy kept in the data folder')).toBeVisible();
    expect(screen.getByText('Supported')).toBeVisible();
    await click('Edit controls');
    expect(store.step).toBe(4);
    cleanup();
    store.step = 6;
    render(SetupReady);
    await click('Start the Launcher');
    expect(mock.api.finishSetup).toHaveBeenCalled();
  });

  it('shows defaults, an unknown build, conflicts and no import', () => {
    store.prefs.keep_private_copy = false;
    store.apk = makeReport({ status: 'unknown', version: null });
    store.draft = makeConfig({
      keys: { ...makeConfig().keys, back: 'F11' },
      window: { width: 800, height: 600, fullscreen: true, vsync: false, ui_scale: 1 }
    });
    render(SetupReady);
    expect(screen.getByText('Fullscreen · 800 × 600 · VSync off · UI 100 %')).toBeVisible();
    expect(screen.getByText('Two actions share a key. Fix it in Settings.')).toBeVisible();
    expect(screen.getByText('Nothing imported')).toBeVisible();
    expect(screen.getByText('Unknown build · experimental')).toBeVisible();
    expect(screen.queryByText('Private copy kept in the data folder')).toBeNull();
    cleanup();
    store.apk = null;
    store.draft = null;
    store.defaults = null;
    render(SetupReady);
    expect(screen.getByText('Default mouse and keys')).toBeVisible();
  });

  it('rail: finished steps are buttons, the rest are not', async () => {
    store.step = 4;
    render(SetupControls);
    const rail = screen.getByRole('navigation', { name: 'Setup steps' });
    await fireEvent.click(within(rail).getByRole('button', { name: /Game file/ }));
    expect(store.step).toBe(2);
    expect(within(rail).queryByRole('button', { name: /Controls/ })).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Back' }));
  });
});
