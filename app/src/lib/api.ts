import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import type {
  ApkReport, AppInfo, BootState, DisplayInfo, ImportSummary, LaunchConfig, Prefs, Snapshot, UpdateInfo, UpdateProgress
} from './types';

/** Thin typed wrapper over the Tauri commands (see src-tauri/src/lib.rs). */
export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  boot: () => invoke<BootState>('boot'),
  saveConfig: (config: LaunchConfig) => invoke<LaunchConfig>('save_config', { config }),
  savePrefs: (prefs: Prefs) => invoke<Prefs>('save_prefs', { prefs }),
  finishSetup: (config: LaunchConfig, prefs: Prefs) => invoke<LaunchConfig>('finish_setup', { config, prefs }),
  checkApk: (path: string, force: boolean) => invoke<ApkReport>('check_apk', { path, force }),
  useApk: (path: string, force: boolean, keepCopy: boolean) =>
    invoke<LaunchConfig>('use_apk', { path, force, keepCopy }),
  normalizeBinding: (text: string) => invoke<string>('normalize_binding', { text }),
  displayInfo: () => invoke<DisplayInfo | null>('display_info'),
  importSaves: (source: string, overwrite: boolean, dataDir: string) =>
    invoke<ImportSummary>('import_saves', { source, overwrite, dataDir }),
  dataSize: (path: string) => invoke<number>('data_size', { path }),
  moveDataDir: (to: string) => invoke<LaunchConfig>('move_data_dir', { to }),
  openPath: (path: string) => invoke<void>('open_path', { path }),
  gameStart: () => invoke<void>('game_start'),
  gameStop: () => invoke<void>('game_stop'),
  gamePoll: (since: number) => invoke<Snapshot>('game_poll', { since }),
  saveLog: (path: string) => invoke<void>('save_log', { path }),
  updateCheck: () => invoke<UpdateInfo | null>('update_check'),
  updateInstall: () => invoke<void>('update_install'),
  updateProgress: () => invoke<UpdateProgress>('update_progress'),
  openUrl: (url: string) => openUrl(url),

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

/** Window chrome, close handling and file drops. */
export const win = {
  minimize: () => getCurrentWindow().minimize(),
  toggleMaximize: () => getCurrentWindow().toggleMaximize(),
  close: () => getCurrentWindow().close(),
  isMaximized: () => getCurrentWindow().isMaximized(),
  onResized: (cb: () => void) => getCurrentWindow().onResized(cb),
  onFocus: (cb: (focused: boolean) => void) => getCurrentWindow().onFocusChanged((e) => cb(e.payload)),
  onCloseRequested: (cb: (prevent: () => void) => void) =>
    getCurrentWindow().onCloseRequested((e) => cb(() => e.preventDefault())),
  onFileDrop: (cb: (paths: string[]) => void) =>
    getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop') cb(e.payload.paths);
    })
};
