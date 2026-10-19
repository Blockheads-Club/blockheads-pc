/** Shapes shared with the Tauri backend (src-tauri/src/lib.rs, crates/bhpc-launcher). Field names are snake_case. */
export type ThemeChoice = 'system' | 'light' | 'dark';
export type Os = 'windows' | 'macos' | 'linux';
export type GameState = 'ready' | 'starting' | 'running' | 'stopped' | 'crashed' | 'missing';

export type FpsPosition = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';

export interface WindowOptions {
  width: number;
  height: number;
  fullscreen: boolean;
  vsync: boolean;
  /** 0.5 to 4; the UI shows percent. */
  ui_scale: number;
  /** Draw a frames-per-second counter in the top-left corner of the game. */
  show_fps: boolean;
  /** Corner of the game picture the counter sits in. */
  fps_position: FpsPosition;
  /** Frames per second at most; 0 = no limit (vsync, when on, still applies). */
  fps_limit: number;
  /** World speed multiplier, 0.25 to 4 (1 = normal; single-player only). */
  game_speed: number;
  /** Run the game's threads in parallel on several CPU cores (off: one at a time, for troubleshooting). */
  parallel_threads: boolean;
}

export interface KeyBindings {
  touch: string;
  pan: string;
  zoom: string;
  pinch: string;
  back: string;
  fullscreen: string;
  hide_ui: string;
}
export type ActionId = keyof KeyBindings;

export interface LaunchConfig {
  apk: string;
  data_dir: string;
  force: boolean;
  window: WindowOptions;
  keys: KeyBindings;
  /** Game controllers drive the game. */
  controller: boolean;
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

/** How a started game runs: inside the launcher window, or in a window of its own. */
export interface Started {
  embedded: boolean;
}

/** Start-up progress of the game runner (see `crates/bhpc-launcher/src/progress.rs`). */
export interface BootProgress {
  /** 0 to 100, never decreasing. */
  percent: number;
  /** Protocol name of the current step. */
  phase: string;
  /** 1 to 6. */
  step: number;
  elapsed_secs: number;
  /** Seconds left once 3 s and 15 % are reached. */
  eta_secs: number | null;
  slow: boolean;
  stalled: boolean;
  /** Seconds each finished step took. */
  step_secs: (number | null)[];
  /** The first frame is up. */
  ready: boolean;
}

export interface Snapshot {
  state: GameState;
  running_secs: number | null;
  last_session_secs: number | null;
  exit_code: number | null;
  signal: number | null;
  lines: string[];
  next: number;
  /** Start-up progress of the running game, or where the last one got to. */
  progress?: BootProgress | null;
  /** The game is drawn inside the launcher window. */
  embedded?: boolean;
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

/** What the hidden main window tells the splash window while it boots. */
export interface SplashState {
  boot: 'loading' | 'slow' | 'error';
  error: string;
  first_run: boolean;
  version: string;
  config_dir: string;
  lang: string;
}
