/** Shapes shared with the Tauri backend (src-tauri/src/lib.rs, crates/bhpc-launcher). Field names are snake_case. */
export type ThemeChoice = 'system' | 'light' | 'dark';
export type Os = 'windows' | 'macos' | 'linux';
export type GameState = 'ready' | 'launching' | 'running' | 'stopped' | 'crashed' | 'missing';

export interface WindowOptions {
  width: number;
  height: number;
  fullscreen: boolean;
  vsync: boolean;
  /** 0.5 to 4; the UI shows percent. */
  ui_scale: number;
}

export interface KeyBindings {
  touch: string;
  pan: string;
  zoom: string;
  pinch: string;
  back: string;
  fullscreen: string;
}
export type ActionId = keyof KeyBindings;

export interface LaunchConfig {
  apk: string;
  data_dir: string;
  force: boolean;
  window: WindowOptions;
  keys: KeyBindings;
}

export interface Prefs {
  language: string;
  theme: ThemeChoice;
  auto_update_check: boolean;
  keep_private_copy: boolean;
  game_sound: boolean;
  mute_in_background: boolean;
}

export type ApkStatus = 'supported' | 'unknown' | 'error';
export type ApkProblem = 'not_blockheads' | 'unsupported' | 'damaged' | 'unreadable';
export interface ApkReport {
  path: string;
  size_bytes: number;
  status: ApkStatus;
  version: string | null;
  problem: ApkProblem | null;
  detail: string;
}

export interface AppInfo {
  version: string;
  os: Os;
  arch: string;
  config_dir: string;
  /** The update check is wired to a release feed in this build. */
  updates_enabled: boolean;
  /** The OS draws the window buttons (macOS traffic lights). */
  native_controls: boolean;
}

export interface BootState {
  /** `null` on the first run. */
  config: LaunchConfig | null;
  defaults: LaunchConfig;
  prefs: Prefs;
}

export interface Snapshot {
  state: GameState;
  running_secs: number | null;
  last_session_secs: number | null;
  exit_code: number | null;
  signal: number | null;
  lines: string[];
  next: number;
}

export interface ImportItem {
  name: string;
  files: number;
  bytes: number;
}
export interface ImportSummary {
  written: number;
  skipped_existing: number;
  ignored: number;
  items: ImportItem[];
}

export interface DisplayInfo {
  width: number;
  height: number;
  /** Device pixel ratio. */
  scale: number;
}

export type UpdateStage = 'idle' | 'checking' | 'uptodate' | 'available' | 'downloading' | 'installing' | 'ready' | 'error';
export interface UpdateInfo {
  version: string;
  size_mb: number;
  notes: string[];
}
export interface UpdateProgress {
  stage: 'downloading' | 'installing' | 'ready';
  percent: number;
  done_mb: number;
  total_mb: number;
  mb_per_s: number;
  eta_s: number;
}
