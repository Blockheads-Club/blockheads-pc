import { vi } from 'vitest';
import { makeConfig, PREFS } from './mock-api';
import { AppStore, DEFAULT_PREFS, store } from './store.svelte';
import { setLang } from './i18n.svelte';
import { toasts } from './ui/toast.svelte';

/** Put the shared store back to its start-up state (tests share one module instance). */
export function resetStore(overrides: Partial<Record<keyof AppStore, unknown>> = {}) {
  const s = store as unknown as Record<string, unknown>;
  s.stopPolling && (store as AppStore).stopPolling();
  Object.assign(s, {
    info: { version: '9.9.9', os: 'linux', arch: 'x86_64', config_dir: '/cfg', updates_enabled: false, native_controls: false },
    config: makeConfig(),
    defaults: makeConfig({ apk: '', data_dir: '/default', window: { width: 1024, height: 768, fullscreen: false, vsync: true, ui_scale: 1 } }),
    prefs: { ...DEFAULT_PREFS, ...PREFS },
    screen: 'home',
    fullscreen: false,
    boot: 'loading',
    bootError: '',
    firstRun: false,
    step: 1,
    section: 'general',
    dialog: null,
    draft: null,
    legalOk: false,
    apk: { path: '/data/game/TheBlockheads.apk', size_bytes: 64_300_000, status: 'supported', version: '1.7.6', problem: null, detail: '' },
    apkChecking: false,
    importing: false,
    importSummary: null,
    importError: '',
    importKind: 'folder',
    importPath: '',
    importOverwrite: false,
    display: { width: 2560, height: 1440, scale: 1.25 },
    dataSize: null,
    game: { state: 'ready', running_secs: null, last_session_secs: null, exit_code: null, signal: null, lines: [], next: 0 },
    log: [],
    update: { stage: 'idle', info: null, progress: null, error: '', checked: false },
    undoKeys: null,
    focusAction: null,
    ...overrides
  });
  setLang('en');
  toasts.items = [];
  localStorage.clear();
  vi.clearAllTimers();
}
