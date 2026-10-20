import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('./api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import type { BootProgress } from './types';
import { makeApi, makeBoot, makeConfig, makeReport, makeSnap, makeWin, PREFS } from './mock-api';
import { MAX_LOG, store } from './store.svelte';
import { resetStore } from './test-utils';
import { theme } from './theme.svelte';
import { i18n } from './i18n.svelte';
import { toasts } from './ui/toast.svelte';

const clipboard = vi.fn(async () => {});
beforeEach(() => {
  vi.useFakeTimers();
  mock.api = makeApi();
  mock.win = makeWin().win;
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: clipboard }, configurable: true });
  clipboard.mockClear();
  resetStore();
});
afterEach(() => {
  store.stopPolling();
  vi.useRealTimers();
});
const msgs = () => toasts.items.map((t) => t.message);

describe('start-up', () => {
  it('goes to setup on the first run with a draft sized for the screen', async () => {
    mock.api.boot = vi.fn(async () => makeBoot({ config: null }));
    await store.init();
    expect(store.screen).toBe('setup');
    expect(store.step).toBe(1);
    expect(store.firstRun).toBe(true);
    expect(store.draft?.window).toMatchObject({ width: 1920, height: 1080 });
    expect(store.cfg).toBe(store.draft);
    // The second start has seen setup before.
    await store.init();
    expect(store.firstRun).toBe(false);
  });

  it('opens Home with the saved configuration and checks the game file', async () => {
    mock.api.appInfo = vi.fn(async () => ({ version: '1', os: 'linux', arch: 'x', config_dir: '/c', updates_enabled: true, native_controls: false }));
    await store.init();
    expect(store.screen).toBe('home');
    expect(store.version).toBe('1.7.6');
    expect(store.cfg).toBe(store.config);
    await vi.waitFor(() => expect(store.dataSize).toBe(5_000_000));
    await vi.waitFor(() => expect(store.update.stage).toBe('available'));
    expect(mock.api.gamePoll).toHaveBeenCalled();
    store.stopPolling();
  });

  it('applies saved preferences and tolerates a missing display and failing update check', async () => {
    mock.api.boot = vi.fn(async () => makeBoot({ prefs: { ...PREFS, language: 'tr', theme: 'dark' } }));
    mock.api.displayInfo = vi.fn(async () => { throw new Error('no monitor'); });
    mock.api.appInfo = vi.fn(async () => ({ version: '1', os: 'linux', arch: 'x', config_dir: '/c', updates_enabled: true, native_controls: false }));
    mock.api.updateCheck = vi.fn(async () => { throw 'offline'; });
    await store.init();
    expect(i18n.lang).toBe('tr');
    expect(theme.choice).toBe('dark');
    expect(store.display).toBeNull();
    await vi.waitFor(() => expect(store.update.stage).toBe('idle'));
    expect(store.update.error).toBe('offline');
  });

  it('reports a failed boot and goes slow after four seconds', async () => {
    let fail: (e: string) => void = () => {};
    mock.api.boot = vi.fn(() => new Promise((_, rej) => (fail = rej)));
    store.screen = 'splash';
    const p = store.init();
    await vi.advanceTimersByTimeAsync(4100);
    expect(store.boot).toBe('slow');
    fail('i/o error: denied');
    await p;
    expect(store.boot).toBe('error');
    expect(store.bootError).toBe('i/o error: denied');
  });

  it('does not mark slow once the app is past the splash', async () => {
    let done: (b: unknown) => void = () => {};
    mock.api.boot = vi.fn(() => new Promise((r) => (done = r)));
    const p = store.init();
    store.screen = 'home';
    await vi.advanceTimersByTimeAsync(4100);
    expect(store.boot).toBe('loading');
    done(makeBoot());
    await p;
  });

  it('treats blocked storage as an already seen setup', async () => {
    const spy = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    await store.init();
    spy.mockRestore();
    expect(store.firstRun).toBe(false);
  });
});

describe('navigation', () => {
  it('moves between screens and clears dialogs', () => {
    store.dialog = 'join';
    store.openSettings('controls');
    expect([store.screen, store.section, store.dialog]).toEqual(['settings', 'controls', null]);
    // Without a page, Settings reopens on the last one.
    store.openSettings();
    expect(store.section).toBe('controls');
    store.go('home');
    expect(store.screen).toBe('home');
  });

  it('guards the setup steps', () => {
    store.screen = 'setup';
    store.step = 1;
    expect(store.canNext).toBe(false);
    store.next();
    expect(store.step).toBe(1);
    store.legalOk = true;
    store.next();
    expect(store.step).toBe(2);
    store.apk = null;
    expect(store.canNext).toBe(false);
    store.apk = makeReport({ status: 'error', problem: 'damaged' });
    expect(store.canNext).toBe(false);
    store.apk = makeReport({ status: 'unknown' });
    store.apkChecking = true;
    expect(store.canNext).toBe(false);
    store.apkChecking = false;
    expect(store.canNext).toBe(true);
    store.next();
    store.step = 5;
    store.importing = true;
    expect(store.canNext).toBe(false);
    store.importing = false;
    store.next();
    expect(store.step).toBe(6);
    store.next();
    expect(store.step).toBe(6);
    store.jump(3);
    expect(store.step).toBe(3);
    store.jump(5);
    expect(store.step).toBe(3);
    store.back();
    store.back();
    store.back();
    expect(store.step).toBe(1);
  });
});

describe('preferences', () => {
  it('applies a change at once and saves it', async () => {
    await store.patchPrefs({ language: 'tr', theme: 'light' });
    expect(i18n.lang).toBe('tr');
    expect(theme.choice).toBe('light');
    expect(mock.api.savePrefs).toHaveBeenCalledWith(expect.objectContaining({ language: 'tr' }));
  });

  it('shows a save failure', async () => {
    mock.api.savePrefs = vi.fn(async () => { throw 'read-only'; });
    await store.patchPrefs({ game_sound: false });
    expect(store.prefs.game_sound).toBe(false);
    expect(msgs()).toEqual(['read-only']);
    expect(toasts.items[0].tone).toBe('bad');
  });
});

describe('game file', () => {
  it('rechecks the configured file', async () => {
    await store.recheckApk();
    expect(mock.api.checkApk).toHaveBeenCalledWith('/data/game/TheBlockheads.apk', false);
    expect(store.apkChecking).toBe(false);
    store.config = null;
    mock.api.checkApk.mockClear();
    await store.recheckApk();
    expect(mock.api.checkApk).not.toHaveBeenCalled();
    store.config = makeConfig();
    mock.api.checkApk = vi.fn(async () => { throw 'gone'; });
    await store.recheckApk();
    expect(msgs()).toEqual(['gone']);
  });

  it('keeps a picked file in the draft only when it is usable', async () => {
    store.screen = 'setup';
    store.draft = makeConfig({ apk: '' });
    await store.pickApk('/dl/good.apk');
    expect(store.draft?.apk).toBe('/dl/good.apk');
    mock.api.checkApk = vi.fn(async (path: string) => makeReport({ path, status: 'error', problem: 'unsupported' }));
    await store.pickApk('/dl/old.apk');
    expect(store.draft?.apk).toBe('/dl/good.apk');
    expect(store.apk?.problem).toBe('unsupported');
    mock.api.checkApk = vi.fn(async () => { throw 'denied'; });
    await store.pickApk('/dl/x.apk');
    expect(msgs()).toEqual(['denied']);
    store.draft = null;
    mock.api.checkApk.mockClear();
    await store.pickApk('/dl/x.apk');
    expect(mock.api.checkApk).not.toHaveBeenCalled();
  });

  it('allows unknown builds in setup and in settings', async () => {
    store.screen = 'setup';
    store.draft = makeConfig();
    await store.setForce(true);
    expect(store.draft?.force).toBe(true);
    expect(mock.api.checkApk).toHaveBeenCalledWith('/data/game/TheBlockheads.apk', true);
    store.apk = null;
    mock.api.checkApk.mockClear();
    await store.setForce(false);
    expect(mock.api.checkApk).not.toHaveBeenCalled();
    store.draft = null;
    await store.setForce(true);

    store.screen = 'settings';
    await store.setForce(true);
    expect(mock.api.saveConfig).toHaveBeenCalledWith(expect.objectContaining({ force: true }));
    expect(mock.api.checkApk).toHaveBeenCalled();
    store.config = null;
    await store.setForce(true);
  });

  it('switches the game file from settings', async () => {
    await store.changeApk('/new.apk');
    expect(mock.api.useApk).toHaveBeenCalledWith('/new.apk', false, true);
    expect(msgs()).toEqual(['Game file changed']);
    mock.api.useApk = vi.fn(async () => { throw 'unknown build'; });
    await store.changeApk('/bad.apk');
    expect(msgs()[1]).toBe('unknown build');
    expect(store.apkChecking).toBe(false);
    store.config = null;
    mock.api.useApk.mockClear();
    await store.changeApk('/x.apk');
    expect(mock.api.useApk).not.toHaveBeenCalled();
  });

  it('browses for a game file', async () => {
    store.screen = 'setup';
    store.draft = makeConfig();
    await store.browseApk();
    expect(mock.api.pickFile).toHaveBeenCalledWith(['apk'], 'Android app');
    expect(mock.api.checkApk).toHaveBeenCalledWith('/dl/TheBlockheads.apk', false);
    store.screen = 'settings';
    await store.browseApk();
    expect(mock.api.useApk).toHaveBeenCalled();
    mock.api.pickFile = vi.fn(async () => null);
    mock.api.useApk.mockClear();
    await store.browseApk();
    expect(mock.api.useApk).not.toHaveBeenCalled();
    mock.api.pickFile = vi.fn(async () => { throw 'dialog failed'; });
    await store.browseApk();
    expect(msgs()).toContain('dialog failed');
  });
});

describe('configuration', () => {
  it('edits the draft in memory during setup', async () => {
    store.screen = 'setup';
    store.draft = makeConfig();
    await store.patchConfig((c) => (c.window.vsync = false));
    expect(store.draft?.window.vsync).toBe(false);
    expect(mock.api.saveConfig).not.toHaveBeenCalled();
    store.draft = null;
    await store.patchConfig(() => {});
  });

  it('saves edits right away elsewhere', async () => {
    await store.patchConfig((c) => (c.window.vsync = false));
    expect(mock.api.saveConfig).toHaveBeenCalledTimes(1);
    expect(store.config?.window.vsync).toBe(false);
    mock.api.saveConfig = vi.fn(async () => { throw 'bad window'; });
    await store.patchConfig((c) => (c.window.width = 1));
    expect(msgs()).toEqual(['bad window']);
    store.config = null;
    mock.api.saveConfig.mockClear();
    await store.patchConfig(() => {});
    expect(mock.api.saveConfig).not.toHaveBeenCalled();
  });

  it('rebinds, undoes and resets key bindings', async () => {
    expect(await store.setBinding('back', 'Backspace')).toBe(true);
    expect(store.config?.keys.back).toBe('Backspace');
    expect(store.undoKeys?.back).toBe('Esc');
    await store.undoBinding();
    expect(store.config?.keys.back).toBe('Esc');
    await store.undoBinding();
    await store.setBinding('pan', 'Middle');
    await store.resetBinding('pan');
    expect(store.config?.keys.pan).toBe('Right');
    await store.setBinding('pan', 'Middle');
    await store.resetKeys();
    expect(store.config?.keys.pan).toBe('Right');
    expect(store.undoKeys).toBeNull();
    mock.api.normalizeBinding = vi.fn(async () => { throw 'invalid binding: F13'; });
    expect(await store.setBinding('pan', 'F13')).toBe(false);
    expect(msgs()).toEqual(['invalid binding: F13']);
    store.config = null;
    expect(await store.setBinding('pan', 'Q')).toBe(false);
    store.defaults = null;
    await store.resetBinding('pan');
    await store.resetKeys();
  });
});

describe('data folder', () => {
  it('measures, moves and opens it', async () => {
    await store.refreshDataSize();
    expect(store.dataSize).toBe(5_000_000);
    mock.api.dataSize = vi.fn(async () => { throw 'x'; });
    await store.refreshDataSize();
    expect(store.dataSize).toBeNull();
    store.config = null;
    mock.api.dataSize.mockClear();
    await store.refreshDataSize();
    expect(mock.api.dataSize).not.toHaveBeenCalled();

    store.config = makeConfig();
    mock.api.dataSize = vi.fn(async () => 7);
    await store.moveData();
    expect(store.config?.data_dir).toBe('/dl/saves');
    expect(store.dataSize).toBe(7);
    expect(msgs()).toEqual(['Data copied to the new folder. The old folder was kept.']);
    mock.api.pickFolder = vi.fn(async () => null);
    mock.api.moveDataDir.mockClear();
    await store.moveData();
    expect(mock.api.moveDataDir).not.toHaveBeenCalled();
    mock.api.pickFolder = vi.fn(async () => '/x');
    mock.api.moveDataDir = vi.fn(async () => { throw 'full disk'; });
    await store.moveData();
    expect(msgs()[1]).toBe('full disk');

    await store.openDataFolder();
    expect(mock.api.openPath).toHaveBeenCalledWith('/dl/saves');
    mock.api.openPath = vi.fn(async () => { throw 'no file manager'; });
    await store.openDataFolder();
    expect(msgs()[2]).toBe('no file manager');
    store.config = null;
    mock.api.openPath.mockClear();
    await store.openDataFolder();
    expect(mock.api.openPath).not.toHaveBeenCalled();
  });
});

describe('finishing setup', () => {
  it('saves the draft and opens Home', async () => {
    store.screen = 'setup';
    store.draft = makeConfig({ apk: '/dl/g.apk' });
    await store.finishSetup();
    expect(store.screen).toBe('home');
    expect(store.draft).toBeNull();
    expect(store.config?.apk).toBe('/data/game/TheBlockheads.apk');
  });

  it('stays in setup on a refusal and ignores a missing draft', async () => {
    store.screen = 'setup';
    store.draft = makeConfig();
    mock.api.finishSetup = vi.fn(async () => { throw 'unknown build'; });
    await store.finishSetup();
    expect(store.screen).toBe('setup');
    expect(msgs()).toEqual(['unknown build']);
    store.draft = null;
    mock.api.finishSetup.mockClear();
    await store.finishSetup();
    expect(mock.api.finishSetup).not.toHaveBeenCalled();
  });
});

describe('import', () => {
  it('browses by source kind', async () => {
    await store.browseImport();
    expect(store.importPath).toBe('/dl/saves');
    store.importKind = 'zip';
    await store.browseImport();
    expect(mock.api.pickFile).toHaveBeenCalledWith(['zip'], 'Zip');
    mock.api.pickFile = vi.fn(async () => null);
    store.importPath = '';
    await store.browseImport();
    expect(store.importPath).toBe('');
    mock.api.pickFolder = vi.fn(async () => { throw 'no dialog'; });
    store.importKind = 'folder';
    await store.browseImport();
    expect(msgs()).toEqual(['no dialog']);
  });

  it('imports, reports and resets', async () => {
    await store.runImport();
    expect(mock.api.importSaves).not.toHaveBeenCalled();
    store.importPath = '/dl/saves';
    store.importOverwrite = true;
    await store.runImport();
    expect(mock.api.importSaves).toHaveBeenCalledWith('/dl/saves', true, '/data');
    expect(store.importSummary?.written).toBe(3);
    expect(store.importing).toBe(false);
    await vi.waitFor(() => expect(store.dataSize).toBe(5_000_000));
    store.resetImport();
    expect([store.importSummary, store.importPath]).toEqual([null, '']);
    store.screen = 'setup';
    store.draft = makeConfig();
    store.importPath = '/dl/saves';
    mock.api.dataSize.mockClear();
    await store.runImport();
    expect(mock.api.dataSize).not.toHaveBeenCalled();
    mock.api.importSaves = vi.fn(async () => { throw 'bad zip'; });
    await store.runImport();
    expect(store.importError).toBe('bad zip');
    expect(store.importSummary).toBeNull();
    store.draft = null;
    store.screen = 'home';
    store.config = null;
    mock.api.importSaves.mockClear();
    await store.runImport();
    expect(mock.api.importSaves).not.toHaveBeenCalled();
  });
});

describe('playing', () => {
  it('starts the game and shows the playing screen', async () => {
    // Play again after a finished session: its 100 % is dropped before the first poll answers.
    store.game = makeSnap({ state: 'stopped', progress: { ...makeSnap().progress, ready: true, percent: 100 } as BootProgress });
    let seen: BootProgress | null | undefined;
    mock.api.gamePoll = vi.fn(async () => {
      seen = store.game.progress;
      return makeSnap({ state: 'starting', running_secs: 0 });
    });
    await store.play();
    expect(seen).toBeNull();
    expect(mock.api.gameStart).toHaveBeenCalled();
    expect(store.screen).toBe('playing');
    expect(store.active).toBe(true);
    // Back to game while it runs does not start a second copy.
    store.screen = 'home';
    mock.api.gameStart.mockClear();
    await store.play();
    expect(mock.api.gameStart).not.toHaveBeenCalled();
    expect(store.screen).toBe('playing');
  });

  it('reports a start failure on Home', async () => {
    mock.api.gameStart = vi.fn(async () => { throw 'cannot start the game: no sidecar'; });
    await store.play();
    expect(store.screen).toBe('home');
    expect(msgs()).toEqual(['cannot start the game: no sidecar']);
  });

  it('follows the process, keeps a bounded log and leaves Playing when it ends', async () => {
    store.screen = 'playing';
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', lines: ['a', 'b'], next: 2 }));
    await store.poll();
    expect(store.log).toEqual(['a', 'b']);
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', lines: [], next: 2 }));
    await store.poll();
    expect(store.log).toEqual(['a', 'b']);
    // A new session restarts the numbering.
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', lines: ['c'], next: 1 }));
    await store.poll();
    expect(store.log).toEqual(['c']);
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', lines: Array.from({ length: MAX_LOG + 5 }, (_, i) => `l${i}`), next: MAX_LOG + 6 }));
    await store.poll();
    expect(store.log).toHaveLength(MAX_LOG);
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'crashed', exit_code: 139, next: MAX_LOG + 6 }));
    await store.poll();
    expect(store.screen).toBe('home');
    expect(store.state).toBe('crashed');
    mock.api.gamePoll = vi.fn(async () => { throw 'busy'; });
    await store.poll();
    expect(store.state).toBe('crashed');
  });

  it('does not poll before the app is ready', async () => {
    store.screen = 'splash';
    await store.poll();
    store.screen = 'setup';
    await store.poll();
    expect(mock.api.gamePoll).not.toHaveBeenCalled();
  });

  it('stops the game', async () => {
    await store.stop();
    expect(mock.api.gameStop).toHaveBeenCalled();
    mock.api.gameStop = vi.fn(async () => { throw 'x'; });
    await store.stop();
    expect(msgs()).toEqual(['x']);
  });

  it('polls on a timer until stopped', async () => {
    store.startPolling();
    store.startPolling();
    await vi.advanceTimersByTimeAsync(2100);
    expect(mock.api.gamePoll).toHaveBeenCalledTimes(2);
    store.stopPolling();
    store.stopPolling();
    await vi.advanceTimersByTimeAsync(3000);
    expect(mock.api.gamePoll).toHaveBeenCalledTimes(2);
  });

  it('toggles fullscreen for the game view only, and reports a failure', async () => {
    await store.toggleFullscreen();
    expect(mock.api.embedFullscreen).not.toHaveBeenCalled();
    store.game = makeSnap({ state: 'running', embedded: true });
    store.fullscreen = true;
    await store.toggleFullscreen();
    expect(mock.api.embedFullscreen).toHaveBeenCalledWith(false);
    mock.api.embedFullscreen = vi.fn(async () => { throw new Error('nope'); });
    await store.toggleFullscreen();
    expect(msgs()).toContain('nope');
  });

  it('quits from the tray, asking first while a game runs and stopping it', async () => {
    store.requestQuit();
    expect(mock.api.appQuit).toHaveBeenCalledTimes(1);
    expect(store.dialog).toBeNull();
    store.game = makeSnap({ state: 'running' });
    store.requestQuit();
    expect(mock.api.appQuit).toHaveBeenCalledTimes(1);
    expect(store.dialog).toBe('close');
    await store.stopAndClose();
    expect(mock.api.gameStop).toHaveBeenCalled();
    expect(mock.api.appQuit).toHaveBeenCalledTimes(2);
    expect(store.dialog).toBeNull();
  });

  it('reports a quit that fails', async () => {
    mock.api.appQuit = vi.fn(async () => { throw new Error('nope'); });
    store.requestQuit();
    await vi.waitFor(() => expect(msgs()).toContain('nope'));
  });
});

describe('log and system info', () => {
  it('copies and saves the log', async () => {
    store.log = ['one', 'two'];
    await store.copyLog();
    expect(clipboard).toHaveBeenCalledWith('one\ntwo');
    expect(msgs()).toEqual(['Log copied']);
    clipboard.mockRejectedValueOnce(new Error('no'));
    await store.copyLog();
    expect(msgs()[1]).toBe('Could not copy to the clipboard');

    await store.saveLog();
    expect(mock.api.saveLog).toHaveBeenCalledWith('/dl/blockheads-pc.log');
    expect(msgs()[2]).toBe('Log saved as blockheads-pc.log');
    mock.api.pickSavePath = vi.fn(async () => null);
    mock.api.saveLog.mockClear();
    await store.saveLog();
    expect(mock.api.saveLog).not.toHaveBeenCalled();
    mock.api.pickSavePath = vi.fn(async () => '/x.log');
    mock.api.saveLog = vi.fn(async () => { throw 'disk full'; });
    await store.saveLog();
    expect(msgs()[3]).toBe('disk full');
  });

  it('copies system info with and without a backend answer', async () => {
    expect(await store.systemInfo()).toBe(true);
    expect(clipboard).toHaveBeenCalledWith('Blockheads PC 9.9.9 · linux x86_64 · game 1.7.6');
    store.info = null;
    store.apk = null;
    await store.systemInfo();
    expect(clipboard).toHaveBeenLastCalledWith('Blockheads PC ? · ? ? · game -');
  });
});

describe('updates', () => {
  it('checks for a new version', async () => {
    await store.checkUpdate();
    expect(store.update).toMatchObject({ stage: 'available', checked: true });
    mock.api.updateCheck = vi.fn(async () => null);
    await store.checkUpdate();
    expect(store.update.stage).toBe('uptodate');
    mock.api.updateCheck = vi.fn(async () => { throw 'offline'; });
    await store.checkUpdate();
    expect(store.update).toMatchObject({ stage: 'error', error: 'offline' });
    await store.checkUpdate(true);
    expect(store.update.stage).toBe('idle');
  });

  it('downloads and installs, polling progress', async () => {
    let finish: () => void = () => {};
    mock.api.updateInstall = vi.fn(() => new Promise<void>((r) => (finish = r)));
    const p = store.installUpdate();
    expect(store.update.stage).toBe('downloading');
    await vi.advanceTimersByTimeAsync(600);
    expect(store.update.progress?.percent).toBe(42);
    mock.api.updateProgress = vi.fn(async () => ({ stage: 'installing', percent: 100, done_mb: 43, total_mb: 43, mb_per_s: 0, eta_s: 0 }));
    await vi.advanceTimersByTimeAsync(600);
    expect(store.update.stage).toBe('installing');
    mock.api.updateProgress = vi.fn(async () => { throw 'x'; });
    await vi.advanceTimersByTimeAsync(600);
    finish();
    await p;
    expect(store.update.stage).toBe('ready');
  });

  it('reports a failed install and lets the user cancel', async () => {
    mock.api.updateInstall = vi.fn(async () => { throw 'connection lost at 63 %'; });
    await store.installUpdate();
    expect(store.update).toMatchObject({ stage: 'error', error: 'connection lost at 63 %' });
    store.update.stage = 'downloading';
    store.cancelUpdate();
    expect(store.update.stage).toBe('available');
    // A cancelled download that finishes later does not claim to be ready.
    let finish: () => void = () => {};
    mock.api.updateInstall = vi.fn(() => new Promise<void>((r) => (finish = r)));
    const p = store.installUpdate();
    store.cancelUpdate();
    finish();
    await p;
    expect(store.update.stage).toBe('available');
  });
});
