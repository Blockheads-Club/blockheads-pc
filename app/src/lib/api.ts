import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import type {
  ApkReport, AppInfo, BootState, DisplayInfo, ImportSummary, LaunchConfig, Prefs, Snapshot, SplashState, Started, UpdateInfo, UpdateProgress
} from './types';
import type {
  AutoUpdate, Browse, CacheOverview, DevMod, InstallProgress, InstallRequest, Overview, Plan
} from './mods/types';

/** Thin typed wrapper over the Tauri commands (see src-tauri/src/lib.rs). */
export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  splashSet: (splash: SplashState) => invoke<void>('splash_set', { splash }),
  splashGet: () => invoke<SplashState | null>('splash_get'),
  splashRetry: () => invoke<void>('splash_retry'),
  appQuit: () => invoke<void>('app_quit'),
  /** The boot work is done: show the main window, close the splash. */
  mainReady: () => invoke<void>('main_ready'),
  onSplashState: (cb: (s: SplashState) => void) => listen<SplashState>('splash-state', (e) => cb(e.payload)),
  onSplashRetry: (cb: () => void) => listen('splash-retry', () => cb()),
  /** Quit from the tray (or the close button when there is no tray): the window decides, see `store.requestQuit`. */
  onQuitRequested: (cb: () => void) => listen('quit-requested', () => cb()),
  /** A page the in-window game shows (self-contained HTML). */
  onGamePage: (cb: (html: string) => void) => listen<string>('game-page', (e) => cb(e.payload)),
  trayLabels: (show: string, quit: string) => invoke<void>('tray_labels', { show, quit }),
  boot: () => invoke<BootState>('boot'),
  saveConfig: (config: LaunchConfig) => invoke<LaunchConfig>('save_config', { config }),
  savePrefs: (prefs: Prefs) => invoke<Prefs>('save_prefs', { prefs }),
  finishSetup: (config: LaunchConfig, prefs: Prefs) => invoke<LaunchConfig>('finish_setup', { config, prefs }),
  checkApk: (path: string, force: boolean) => invoke<ApkReport>('check_apk', { path, force }),
  useApk: (path: string, force: boolean, keepCopy: boolean) =>
    invoke<LaunchConfig>('use_apk', { path, force, keepCopy }),
  normalizeBinding: (text: string) => invoke<string>('normalize_binding', { text }),
  displayInfo: () => invoke<DisplayInfo | null>('display_info'),
  /** Size the launcher window so its game stage is `width` x `height` (it leaves maximized; fullscreen stays). */
  resizeWindow: (width: number, height: number) => invoke<void>('window_resize', { width, height }),
  importSaves: (source: string, overwrite: boolean, dataDir: string) =>
    invoke<ImportSummary>('import_saves', { source, overwrite, dataDir }),
  dataSize: (path: string) => invoke<number>('data_size', { path }),
  moveDataDir: (to: string) => invoke<LaunchConfig>('move_data_dir', { to }),
  openPath: (path: string) => invoke<void>('open_path', { path }),
  gameStart: () => invoke<Started>('game_start'),
  gameStop: () => invoke<void>('game_stop'),
  /** The page tells the backend where the in-window game stage starts (pixels from the top; 0 none, -1 hide it). */
  embedLayout: (top: number) => invoke<void>('embed_layout', { top }),
  embedReveal: () => invoke<void>('embed_reveal'),
  embedFullscreen: (on: boolean) => invoke<void>('embed_fullscreen', { on }),
  gamePoll: (since: number) => invoke<Snapshot>('game_poll', { since }),
  saveLog: (path: string) => invoke<void>('save_log', { path }),
  updateCheck: () => invoke<UpdateInfo | null>('update_check'),
  updateInstall: () => invoke<void>('update_install'),
  updateProgress: () => invoke<UpdateProgress>('update_progress'),
  updateRestart: () => invoke<void>('update_restart'),
  openUrl: (url: string) => openUrl(url),

  /** Client mods (src-tauri/src/mods.rs). Failures reject with a `ModError` object. */
  modsOverview: () => invoke<Overview>('mods_overview'),
  modsIndex: (refresh: boolean) => invoke<Browse>('mods_index', { refresh }),
  modsPlan: (id: string, version: string | null) => invoke<Plan>('mods_plan', { id, version }),
  modsInspectFile: (path: string) => invoke<Plan>('mods_inspect_file', { path }),
  modsInspectLink: (url: string) => invoke<Plan>('mods_inspect_link', { url }),
  modsInstall: (req: InstallRequest) => invoke<string[]>('mods_install', { req }),
  onModsInstall: (cb: (p: InstallProgress) => void) => listen<InstallProgress>('mods-install', (e) => cb(e.payload)),
  modsSetEnabled: (id: string, on: boolean) => invoke<void>('mods_set_enabled', { id, on }),
  modsSetGrant: (id: string, perm: string, on: boolean) => invoke<void>('mods_set_grant', { id, perm, on }),
  modsRemove: (id: string) => invoke<void>('mods_remove', { id }),
  modsAutoUpdate: () => invoke<AutoUpdate>('mods_auto_update'),
  modsServers: () => invoke<CacheOverview>('mods_servers'),
  modsClearServer: (key: string) => invoke<CacheOverview>('mods_clear_server', { key }),
  modsClearServers: () => invoke<CacheOverview>('mods_clear_servers'),
  modsDevInspect: (path: string) => invoke<DevMod>('mods_dev_inspect', { path }),
  modsDevReload: (path: string) => invoke<void>('mods_dev_reload', { path }),
  /** Changes when a file in one of the folders changes (live reload). */
  modsDevFingerprint: (paths: string[]) => invoke<string>('mods_dev_fingerprint', { paths }),
  /** Starts the game and joins this server from its main menu. */
  modsDevJoin: (address: string) => invoke<Started>('mods_dev_join', { address }),

  /** Native pickers; `null` when the user cancels. */
  async pickFile(extensions: string[], name: string): Promise<string | null> {
    const r = await openDialog({ multiple: false, directory: false, filters: [{ name, extensions }] });
    return typeof r === 'string' ? r : null;
  },
  async pickFolder(): Promise<string | null> {
    const r = await openDialog({ multiple: false, directory: true });
    return typeof r === 'string' ? r : null;
  },
  async pickSavePath(defaultPath: string): Promise<string | null> {
    return (await saveDialog({ defaultPath })) ?? null;
  }
};

/** Window chrome and file drops. Closing hides the window to the tray (backend, `tray.rs`). */
export const win = {
  minimize: () => getCurrentWindow().minimize(),
  toggleMaximize: () => getCurrentWindow().toggleMaximize(),
  close: () => getCurrentWindow().close(),
  isMaximized: () => getCurrentWindow().isMaximized(),
  isFullscreen: () => getCurrentWindow().isFullscreen(),
  onResized: (cb: () => void) => getCurrentWindow().onResized(cb),
  onFocus: (cb: (focused: boolean) => void) => getCurrentWindow().onFocusChanged((e) => cb(e.payload)),
  onFileDrop: (cb: (paths: string[]) => void) =>
    getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop') cb(e.payload.paths);
    })
};
