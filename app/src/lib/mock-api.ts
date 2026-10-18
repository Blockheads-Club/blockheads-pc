import { vi } from 'vitest';
import type { ApkReport, BootState, LaunchConfig, Prefs, Snapshot } from './types';

export const KEYS = { touch: 'Left', pan: 'Right', zoom: 'Wheel', pinch: 'Ctrl+Left', back: 'Esc', fullscreen: 'F11' };

export function makeConfig(over: Partial<LaunchConfig> = {}): LaunchConfig {
  return {
    apk: '/data/game/TheBlockheads.apk',
    data_dir: '/data',
    force: false,
    window: { width: 1600, height: 900, fullscreen: false, vsync: true, ui_scale: 1.25 },
    keys: { ...KEYS },
    ...over
  };
}

export const PREFS: Prefs = {
  language: 'en',
  theme: 'system',
  auto_update_check: true,
  keep_private_copy: true,
  game_sound: true,
  mute_in_background: false
};

export function makeSnap(over: Partial<Snapshot> = {}): Snapshot {
  return { state: 'ready', running_secs: null, last_session_secs: null, exit_code: null, signal: null, lines: [], next: 0, ...over };
}

export function makeReport(over: Partial<ApkReport> = {}): ApkReport {
  return { path: '/dl/TheBlockheads.apk', size_bytes: 64_300_000, status: 'supported', version: '1.7.6', problem: null, detail: '', ...over };
}

export function makeBoot(over: Partial<BootState> = {}): BootState {
  return { config: makeConfig(), defaults: makeConfig({ apk: '', data_dir: '/default-data', window: { width: 1024, height: 768, fullscreen: false, vsync: true, ui_scale: 1 } }), prefs: { ...PREFS }, ...over };
}

/** A fake of everything `$lib/api` exports. */
export function makeApi(over: Record<string, unknown> = {}) {
  return {
    appInfo: vi.fn(async () => ({ version: '9.9.9', os: 'linux', arch: 'x86_64', config_dir: '/cfg', updates_enabled: false, native_controls: false })),
    boot: vi.fn(async () => makeBoot()),
    saveConfig: vi.fn(async (c: LaunchConfig) => c),
    savePrefs: vi.fn(async (p: Prefs) => p),
    finishSetup: vi.fn(async (c: LaunchConfig) => ({ ...c, apk: '/data/game/TheBlockheads.apk' })),
    checkApk: vi.fn(async (path: string) => makeReport({ path })),
    useApk: vi.fn(async (path: string, force: boolean) => makeConfig({ apk: path, force })),
    normalizeBinding: vi.fn(async (t: string) => t),
    displayInfo: vi.fn(async () => ({ width: 2560, height: 1440, scale: 1.25 })),
    importSaves: vi.fn(async () => ({ written: 3, skipped_existing: 1, ignored: 2, items: [{ name: 'saves/a', files: 3, bytes: 3000 }] })),
    dataSize: vi.fn(async () => 5_000_000),
    moveDataDir: vi.fn(async (to: string) => makeConfig({ data_dir: to })),
    openPath: vi.fn(async () => {}),
    gameStart: vi.fn(async () => {}),
    gameStop: vi.fn(async () => {}),
    gamePoll: vi.fn(async () => makeSnap()),
    saveLog: vi.fn(async () => {}),
    updateCheck: vi.fn(async () => ({ version: '0.2.0', size_mb: 43, notes: ['one', 'two'] })),
    updateInstall: vi.fn(async () => {}),
    updateProgress: vi.fn(async () => ({ stage: 'downloading', percent: 42, done_mb: 18, total_mb: 43, mb_per_s: 1.4, eta_s: 30 })),
    openUrl: vi.fn(async () => {}),
    pickFile: vi.fn(async () => '/dl/TheBlockheads.apk' as string | null),
    pickFolder: vi.fn(async () => '/dl/saves' as string | null),
    pickSavePath: vi.fn(async () => '/dl/blockheads-pc.log' as string | null),
    ...over
  };
}

/** A fake of `win`; `closeRequested` and `dropped` hold the handlers the app registered. */
export function makeWin(over: Record<string, unknown> = {}) {
  const hooks: { close?: (prevent: () => void) => void; drop?: (paths: string[]) => void; resize?: () => void } = {};
  const win = {
    minimize: vi.fn(async () => {}),
    toggleMaximize: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
    isMaximized: vi.fn(async () => false),
    isFullscreen: vi.fn(async () => false),
    onResized: vi.fn(async (cb: () => void) => {
      hooks.resize = cb;
      return () => {};
    }),
    onFocus: vi.fn(async () => () => {}),
    onCloseRequested: vi.fn(async (cb: (prevent: () => void) => void) => {
      hooks.close = cb;
      return () => {};
    }),
    onFileDrop: vi.fn(async (cb: (paths: string[]) => void) => {
      hooks.drop = cb;
      return () => {};
    }),
    ...over
  };
  return { win, hooks };
}
