import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.hoisted(() => vi.fn(async () => null));
const win = vi.hoisted(() => ({
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  isMaximized: vi.fn(),
  isFullscreen: vi.fn(),
  onResized: vi.fn(),
  onFocusChanged: vi.fn()
}));
const webview = vi.hoisted(() => ({ onDragDropEvent: vi.fn() }));
const dialog = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }));
const event = vi.hoisted(() => ({ listen: vi.fn(async (_n: string, cb: (e: { payload: unknown }) => void) => cb({ payload: 'p' })) }));
const opener = vi.hoisted(() => ({ openUrl: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => win }));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => webview }));
vi.mock('@tauri-apps/plugin-dialog', () => dialog);
vi.mock('@tauri-apps/api/event', () => event);
vi.mock('@tauri-apps/plugin-opener', () => opener);

import { api, win as winApi } from './api';
import { makeConfig, PREFS } from './mock-api';

describe('api', () => {
  beforeEach(() => {
    invoke.mockClear();
    vi.clearAllMocks();
  });

  it('maps every call onto its Tauri command and arguments', async () => {
    const c = makeConfig();
    await api.appInfo();
    await api.splashSet({ boot: 'loading', error: '', first_run: false, version: '1', config_dir: '/c', lang: 'en' });
    await api.splashGet();
    await api.splashRetry();
    await api.appQuit();
    await api.trayLabels('Show', 'Quit');
    await api.mainReady();
    await api.boot();
    await api.saveConfig(c);
    await api.savePrefs(PREFS);
    await api.finishSetup(c, PREFS);
    await api.checkApk('/a.apk', true);
    await api.useApk('/a.apk', false, true);
    await api.normalizeBinding('ctrl+q');
    await api.displayInfo();
    await api.resizeWindow(1280, 720);
    await api.importSaves('/src', true, '/data');
    await api.dataSize('/data');
    await api.moveDataDir('/new');
    await api.openPath('/data');
    await api.gameStart();
    await api.gameStop();
    await api.gamePoll(4);
    await api.embedLayout(84);
    await api.embedReveal();
    await api.embedFullscreen(true);
    await api.saveLog('/x.log');
    await api.updateCheck();
    await api.updateInstall();
    await api.updateProgress();
    expect(invoke.mock.calls.map((x: unknown[]) => x[0])).toEqual([
      'app_info', 'splash_set', 'splash_get', 'splash_retry', 'app_quit', 'tray_labels', 'main_ready', 'boot', 'save_config', 'save_prefs', 'finish_setup', 'check_apk', 'use_apk', 'normalize_binding',
      'display_info', 'window_resize', 'import_saves', 'data_size', 'move_data_dir', 'open_path', 'game_start', 'game_stop', 'game_poll',
      'embed_layout', 'embed_reveal', 'embed_fullscreen', 'save_log', 'update_check', 'update_install', 'update_progress'
    ]);
    expect(invoke).toHaveBeenCalledWith('use_apk', { path: '/a.apk', force: false, keepCopy: true });
    expect(invoke).toHaveBeenCalledWith('import_saves', { source: '/src', overwrite: true, dataDir: '/data' });
    expect(invoke).toHaveBeenCalledWith('game_poll', { since: 4 });
    expect(invoke).toHaveBeenCalledWith('embed_layout', { top: 84 });
    expect(invoke).toHaveBeenCalledWith('window_resize', { width: 1280, height: 720 });
    expect(invoke).toHaveBeenCalledWith('embed_fullscreen', { on: true });
  });

  it('subscribes to the splash events', async () => {
    const seen: unknown[] = [];
    await api.onSplashState((s) => seen.push(s));
    await api.onSplashRetry(() => seen.push('retry'));
    await api.onQuitRequested(() => seen.push('quit'));
    expect(seen).toEqual(['p', 'retry', 'quit']);
    expect(event.listen.mock.calls.map((c) => c[0])).toEqual(['splash-state', 'splash-retry', 'quit-requested']);
  });

  it('opens links and the native pickers', async () => {
    await api.openUrl('https://x');
    expect(opener.openUrl).toHaveBeenCalledWith('https://x');
    dialog.open.mockResolvedValueOnce('/f.apk').mockResolvedValueOnce(['/not', '/single']).mockResolvedValueOnce('/dir').mockResolvedValueOnce(null);
    expect(await api.pickFile(['apk'], 'Android app')).toBe('/f.apk');
    expect(dialog.open).toHaveBeenCalledWith({ multiple: false, directory: false, filters: [{ name: 'Android app', extensions: ['apk'] }] });
    expect(await api.pickFile(['apk'], 'x')).toBeNull();
    expect(await api.pickFolder()).toBe('/dir');
    expect(await api.pickFolder()).toBeNull();
    dialog.save.mockResolvedValueOnce('/s.log').mockResolvedValueOnce(null);
    expect(await api.pickSavePath('a.log')).toBe('/s.log');
    expect(await api.pickSavePath('a.log')).toBeNull();
  });

  it('wraps the window', async () => {
    await winApi.minimize();
    await winApi.toggleMaximize();
    await winApi.close();
    await winApi.isMaximized();
    await winApi.isFullscreen();
    expect([win.minimize, win.toggleMaximize, win.close, win.isMaximized, win.isFullscreen].every((f) => f.mock.calls.length === 1)).toBe(true);
    const cb = vi.fn();
    await winApi.onResized(cb);
    expect(win.onResized).toHaveBeenCalledWith(cb);

    const focus = vi.fn();
    win.onFocusChanged.mockImplementation(async (h: (e: { payload: boolean }) => void) => h({ payload: true }));
    await winApi.onFocus(focus);
    expect(focus).toHaveBeenCalledWith(true);

    const dropped = vi.fn();
    webview.onDragDropEvent.mockImplementation(async (h: (e: { payload: unknown }) => void) => {
      h({ payload: { type: 'over' } });
      h({ payload: { type: 'drop', paths: ['/a.apk'] } });
    });
    await winApi.onFileDrop(dropped);
    expect(dropped).toHaveBeenCalledTimes(1);
    expect(dropped).toHaveBeenCalledWith(['/a.apk']);
  });
});
