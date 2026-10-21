import { api } from './api';
import { detectLang, errText, setLang, t } from './i18n.svelte';
import type { SettingsPage } from './settings-index';
import { setTheme } from './theme.svelte';
import { toast } from './ui/toast.svelte';
import type {
  ActionId, ApkReport, AppInfo, DisplayInfo, GameState, ImportSummary, LaunchConfig, Prefs, Snapshot,
  UpdateInfo, UpdateProgress, UpdateStage
} from './types';
import { baseName, copyText, MIN_EDGE, recommendedSize, stageMatches, stageSize } from './util';

export type Screen = 'splash' | 'setup' | 'home' | 'playing' | 'settings';
export type Section = SettingsPage;
export type DialogName = 'join' | 'update' | 'logs' | 'close' | 'import' | 'resetKeys' | null;
export const SETUP_STEPS = 6;
export const MAX_LOG = 5000;
export const REPO_URL = 'https://github.com/Blockheads-Club/blockheads-pc';
export const GUIDE_URL = REPO_URL;
const SEEN_KEY = 'bhpc.setup-seen';
/** A window resize counts once it has rested this long (dragging a border fires many). */
const RESIZE_SETTLE_MS = 300;

export const DEFAULT_PREFS: Prefs = {
  language: 'en',
  theme: 'system',
  auto_update_check: true,
  keep_private_copy: true,
  game_sound: true,
  mute_in_background: false
};

const IDLE: Snapshot = {
  state: 'ready', running_secs: null, last_session_secs: null, exit_code: null, signal: null, lines: [], next: 0
};

const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

/** Shared UI state; one instance for the whole app. */
export class AppStore {
  info = $state<AppInfo | null>(null);
  /** The saved configuration; `null` until setup has finished. */
  config = $state<LaunchConfig | null>(null);
  defaults = $state<LaunchConfig | null>(null);
  prefs = $state<Prefs>({ ...DEFAULT_PREFS });
  screen = $state<Screen>('splash');
  boot = $state<'loading' | 'slow' | 'error'>('loading');
  bootError = $state('');
  firstRun = $state(false);
  step = $state(1);
  /** The Settings page shown; kept while the app runs, so Settings reopens where it was left. */
  section = $state<Section>('game');
  dialog = $state<DialogName>(null);
  /** The setup draft; the saved config takes over when setup finishes. */
  draft = $state<LaunchConfig | null>(null);
  legalOk = $state(false);
  apk = $state<ApkReport | null>(null);
  apkChecking = $state(false);
  importing = $state(false);
  importSummary = $state<ImportSummary | null>(null);
  importError = $state('');
  importKind = $state<'folder' | 'zip'>('folder');
  importPath = $state('');
  importOverwrite = $state(false);
  display = $state<DisplayInfo | null>(null);
  dataSize = $state<number | null>(null);
  game = $state<Snapshot>({ ...IDLE });
  /** The window is fullscreen (the title bar and strip are hidden while playing). */
  fullscreen = $state(false);
  log = $state<string[]>([]);
  update = $state<{ stage: UpdateStage; info: UpdateInfo | null; progress: UpdateProgress | null; error: string; checked: boolean }>({
    stage: 'idle', info: null, progress: null, error: '', checked: false
  });
  /** Controls: the previous bindings, for "Undo last change". */
  undoKeys = $state<LaunchConfig['keys'] | null>(null);
  /** Controls: open the capture prompt for this action (set by the conflict banner). */
  focusAction = $state<ActionId | null>(null);
  private logNext = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private fast: ReturnType<typeof setInterval> | undefined;
  private slow: ReturnType<typeof setTimeout> | undefined;
  private resizing: ReturnType<typeof setTimeout> | undefined;
  private updTimer: ReturnType<typeof setInterval> | undefined;

  /** The configuration being edited: the draft during setup, the saved one afterwards. */
  get cfg(): LaunchConfig | null {
    return this.screen === 'setup' ? this.draft : this.config;
  }

  get state(): GameState {
    return this.game.state;
  }

  /** The running game is drawn inside this window (not in one of its own). */
  get embedded(): boolean {
    return this.active && this.game.embedded === true;
  }

  /** The game is booting: the loading screen shows until its first frame. */
  get starting(): boolean {
    return this.state === 'starting';
  }

  /** The last start ended before the first frame (crash, or stuck for a minute): the loading screen shows the error. */
  get bootFailed(): boolean {
    const p = this.game.progress;
    return this.state === 'crashed' && !!p && !p.ready;
  }

  get active(): boolean {
    return this.state === 'starting' || this.state === 'running';
  }

  get version(): string {
    return this.apk?.version ?? '';
  }

  get canNext(): boolean {
    if (this.step === 1) return this.legalOk;
    if (this.step === 2) return !!this.apk && this.apk.status !== 'error' && !this.apkChecking;
    if (this.step === 5) return !this.importing;
    return true;
  }

  async init() {
    this.boot = 'loading';
    this.bootError = '';
    this.firstRun = !this.seenSetup();
    clearTimeout(this.slow);
    this.slow = setTimeout(() => {
      if (this.screen === 'splash' && this.boot === 'loading') this.boot = 'slow';
    }, 4000);
    try {
      this.info = await api.appInfo();
      const b = await api.boot();
      this.defaults = b.defaults;
      this.applyPrefs(b.prefs_saved ? b.prefs : { ...b.prefs, language: detectLang(navigator.languages) });
      this.config = b.config;
      this.display = await api.displayInfo().catch(() => null);
      if (b.config) {
        this.screen = 'home';
        await this.recheckApk();
        void this.refreshDataSize();
        if (this.prefs.auto_update_check && this.info.updates_enabled) void this.checkUpdate(true);
      } else {
        this.draft = clone(b.defaults);
        const [w, h] = recommendedSize(this.display);
        this.draft.window.width = w;
        this.draft.window.height = h;
        this.screen = 'setup';
        this.step = 1;
      }
      // The launcher window is the game window: it starts at the chosen size.
      const w = this.cfg!.window;
      void api.resizeWindow(w.width, w.height).catch(() => {});
      await this.poll();
      this.startPolling();
    } catch (e) {
      this.boot = 'error';
      this.bootError = errText(e);
    } finally {
      clearTimeout(this.slow);
    }
  }

  private seenSetup(): boolean {
    try {
      const seen = localStorage.getItem(SEEN_KEY) === '1';
      localStorage.setItem(SEEN_KEY, '1');
      return seen;
    } catch {
      return true;
    }
  }

  fail(e: unknown) {
    toast(errText(e), undefined, 'bad');
  }

  go(screen: Screen) {
    this.screen = screen;
    this.dialog = null;
  }

  openSettings(section?: Section) {
    if (section) this.section = section;
    this.go('settings');
  }

  // ---- preferences -------------------------------------------------------------------------------------------

  private applyPrefs(p: Prefs) {
    this.prefs = p;
    setLang(p.language);
    setTheme(p.theme);
  }

  async patchPrefs(patch: Partial<Prefs>) {
    const next = { ...this.prefs, ...patch };
    this.applyPrefs(next);
    try {
      this.prefs = await api.savePrefs(next);
    } catch (e) {
      this.fail(e);
    }
  }

  // ---- game file ---------------------------------------------------------------------------------------------

  /** Reads the version of the configured game file (Home, Settings). */
  async recheckApk() {
    const cfg = this.cfg;
    if (!cfg) return;
    this.apkChecking = true;
    try {
      this.apk = await api.checkApk(cfg.apk, cfg.force);
    } catch (e) {
      this.fail(e);
    } finally {
      this.apkChecking = false;
    }
  }

  /** Setup: check a file the user picked or dropped. */
  async pickApk(path: string) {
    if (!this.draft) return;
    this.apkChecking = true;
    try {
      const report = await api.checkApk(path, this.draft.force);
      this.apk = report;
      if (report.status !== 'error') this.draft.apk = path;
    } catch (e) {
      this.fail(e);
    } finally {
      this.apkChecking = false;
    }
  }

  async setForce(on: boolean) {
    const cfg = this.cfg;
    if (!cfg) return;
    if (this.screen === 'setup') {
      cfg.force = on;
      if (this.apk) await this.pickApk(this.apk.path);
    } else {
      await this.patchConfig((c) => (c.force = on));
      await this.recheckApk();
    }
  }

  /** Settings: switch to another game file. */
  async changeApk(path: string) {
    if (!this.config) return;
    this.apkChecking = true;
    try {
      this.config = await api.useApk(path, this.config.force, this.prefs.keep_private_copy);
      await this.recheckApk();
      toast(t('set.apkChanged'));
    } catch (e) {
      this.fail(e);
    } finally {
      this.apkChecking = false;
    }
  }

  async browseApk() {
    try {
      const path = await api.pickFile(['apk'], 'Android app');
      if (!path) return;
      if (this.screen === 'setup') await this.pickApk(path);
      else await this.changeApk(path);
    } catch (e) {
      this.fail(e);
    }
  }

  // ---- configuration -----------------------------------------------------------------------------------------

  /** Edit the config being edited. Setup keeps the draft in memory; elsewhere the change is saved right away. */
  async patchConfig(fn: (c: LaunchConfig) => void) {
    if (this.screen === 'setup') {
      if (this.draft) fn(this.draft);
      return;
    }
    if (!this.config) return;
    const next = clone(this.config);
    fn(next);
    try {
      this.config = await api.saveConfig(next);
    } catch (e) {
      this.fail(e);
    }
  }

  /** Settings: the window size. The launcher window takes it at once. */
  async setWindowSize(width: number, height: number) {
    await this.patchConfig((c) => {
      c.window.width = width;
      c.window.height = height;
    });
    await api.resizeWindow(width, height).catch(() => {});
  }

  /** The launcher window was resized (inside `width` x `height` CSS pixels; `square`: maximized or fullscreen). Once it
   * rests, a size the configured one does not explain (the user dragged a border) becomes the configured size, so
   * the settings show it (as a custom size) and the game starts with it. */
  windowResized(width: number, height: number, square: boolean) {
    clearTimeout(this.resizing);
    if (square) return;
    this.resizing = setTimeout(() => {
      const cfg = this.cfg;
      const stage = stageSize(width, height);
      if (!cfg || this.screen === 'splash' || Math.min(...stage) < MIN_EDGE) return;
      if (stageMatches([cfg.window.width, cfg.window.height], stage)) return;
      void this.patchConfig((c) => {
        [c.window.width, c.window.height] = stage;
      });
    }, RESIZE_SETTLE_MS);
  }

  async setController(on: boolean) {
    await this.patchConfig((c) => (c.controller = on));
  }

  async setShowDpad(on: boolean) {
    await this.patchConfig((c) => (c.show_dpad = on));
  }

  async setPadTileCursor(on: boolean) {
    await this.patchConfig((c) => (c.pad_tile_cursor = on));
  }

  async setBinding(action: ActionId, text: string): Promise<boolean> {
    const cfg = this.cfg;
    if (!cfg) return false;
    let normal: string;
    try {
      normal = await api.normalizeBinding(text);
    } catch (e) {
      this.fail(e);
      return false;
    }
    this.undoKeys = { ...cfg.keys };
    await this.patchConfig((c) => (c.keys[action] = normal));
    return true;
  }

  async undoBinding() {
    const back = this.undoKeys;
    this.undoKeys = null;
    if (back) await this.patchConfig((c) => (c.keys = back));
  }

  async resetBinding(action: ActionId) {
    const d = this.defaults;
    if (d) await this.patchConfig((c) => (c.keys[action] = d.keys[action]));
  }

  async resetKeys() {
    const d = this.defaults;
    this.undoKeys = null;
    if (d) await this.patchConfig((c) => (c.keys = clone(d.keys)));
  }

  async refreshDataSize() {
    const dir = this.config?.data_dir;
    if (!dir) return;
    this.dataSize = await api.dataSize(dir).catch(() => null);
  }

  async moveData() {
    try {
      const to = await api.pickFolder();
      if (!to) return;
      this.config = await api.moveDataDir(to);
      await this.refreshDataSize();
      toast(t('set.dataMoved'));
    } catch (e) {
      this.fail(e);
    }
  }

  async openDataFolder() {
    const dir = this.cfg?.data_dir;
    if (!dir) return;
    try {
      await api.openPath(dir);
    } catch (e) {
      this.fail(e);
    }
  }

  // ---- setup flow --------------------------------------------------------------------------------------------

  next() {
    if (!this.canNext || this.step >= SETUP_STEPS) return;
    this.step++;
  }

  back() {
    if (this.step > 1) this.step--;
  }

  /** Setup rail: only finished steps are reachable. */
  jump(step: number) {
    if (step < this.step) this.step = step;
  }

  async finishSetup() {
    if (!this.draft) return;
    try {
      this.config = await api.finishSetup(this.draft, this.prefs);
      this.draft = null;
      this.screen = 'home';
      await this.recheckApk();
      void this.refreshDataSize();
    } catch (e) {
      this.fail(e);
    }
  }

  async browseImport() {
    try {
      const path = this.importKind === 'folder' ? await api.pickFolder() : await api.pickFile(['zip'], 'Zip');
      if (path) this.importPath = path;
    } catch (e) {
      this.fail(e);
    }
  }

  async runImport() {
    const dir = this.cfg?.data_dir;
    if (!dir || !this.importPath) return;
    this.importing = true;
    this.importError = '';
    this.importSummary = null;
    try {
      this.importSummary = await api.importSaves(this.importPath, this.importOverwrite, dir);
      if (this.screen !== 'setup') void this.refreshDataSize();
    } catch (e) {
      this.importError = errText(e);
    } finally {
      this.importing = false;
    }
  }

  resetImport() {
    this.importSummary = null;
    this.importError = '';
    this.importPath = '';
  }

  // ---- game --------------------------------------------------------------------------------------------------

  async play() {
    if (this.active) {
      this.screen = 'playing';
      return;
    }
    try {
      await api.gameStart();
      // The loading screen starts at 0 %, not where the last session ended, until the first poll comes back.
      this.game = { ...this.game, state: 'starting', progress: null };
      this.screen = 'playing';
      this.dialog = null;
      await this.poll();
    } catch (e) {
      this.fail(e);
    }
  }

  async stop() {
    try {
      await api.gameStop();
      await this.poll();
    } catch (e) {
      this.fail(e);
    }
  }

  async poll() {
    if (this.screen === 'splash' || this.screen === 'setup') return;
    try {
      const s = await api.gamePoll(this.logNext);
      if (s.next < this.logNext) this.log = [];
      this.logNext = s.next;
      if (s.lines.length) this.log = [...this.log, ...s.lines].slice(-MAX_LOG);
      this.game = s;
      if (this.screen === 'playing' && !this.active && !this.bootFailed) this.screen = 'home';
      this.speedPolling();
    } catch {
      // The backend may be busy shutting down; the next poll tries again.
    }
  }

  /** Poll four times a second while the game boots so the percentage moves smoothly. */
  private speedPolling() {
    if (this.starting && !this.fast) {
      this.fast = setInterval(() => void this.poll(), 250);
    } else if (!this.starting && this.fast) {
      clearInterval(this.fast);
      this.fast = undefined;
    }
  }

  startPolling() {
    this.stopPolling();
    this.timer = setInterval(() => void this.poll(), 1000);
  }

  stopPolling() {
    clearInterval(this.timer);
    clearInterval(this.updTimer);
    clearInterval(this.fast);
    this.timer = this.updTimer = this.fast = undefined;
  }

  async copyLog() {
    toast((await copyText(this.log.join('\n'))) ? t('logs.copied') : t('common.copyFailed'));
  }

  async saveLog() {
    try {
      const path = await api.pickSavePath('blockheads-pc.log');
      if (!path) return;
      await api.saveLog(path);
      toast(t('logs.saved', { name: baseName(path) }));
    } catch (e) {
      this.fail(e);
    }
  }

  /** The game fullscreen on or off (only while it is shown in the window; the launcher itself never goes fullscreen). */
  async toggleFullscreen() {
    if (!this.embedded) return;
    try {
      await api.embedFullscreen(!this.fullscreen);
    } catch (e) {
      this.fail(e);
    }
  }

  /** Quit from the tray: ask first while a game runs. */
  requestQuit() {
    if (this.active) this.dialog = 'close';
    else void api.appQuit().catch((e) => this.fail(e));
  }

  async stopAndClose() {
    await this.stop();
    this.dialog = null;
    await api.appQuit();
  }

  async systemInfo(): Promise<boolean> {
    const i = this.info;
    const text = `Blockheads PC ${i?.version ?? '?'} · ${i?.os ?? '?'} ${i?.arch ?? '?'} · game ${this.version || '-'}`;
    return copyText(text);
  }

  // ---- updates (UI only until a release feed exists) ---------------------------------------------------------

  async checkUpdate(quiet = false) {
    this.update.stage = 'checking';
    this.update.error = '';
    try {
      const info = await api.updateCheck();
      this.update.checked = true;
      if (info) {
        this.update.info = info;
        this.update.stage = 'available';
      } else {
        this.update.stage = 'uptodate';
      }
    } catch (e) {
      this.update.stage = quiet ? 'idle' : 'error';
      this.update.error = errText(e);
    }
  }

  async installUpdate() {
    this.update.stage = 'downloading';
    this.update.error = '';
    this.updTimer = setInterval(() => void this.pollUpdate(), 500);
    try {
      await api.updateInstall();
      if (this.update.stage === 'downloading' || this.update.stage === 'installing') this.update.stage = 'ready';
    } catch (e) {
      this.update.stage = 'error';
      this.update.error = errText(e);
    } finally {
      clearInterval(this.updTimer);
      this.updTimer = undefined;
    }
  }

  private async pollUpdate() {
    try {
      const p = await api.updateProgress();
      this.update.progress = p;
      if (p.stage === 'installing' && this.update.stage === 'downloading') this.update.stage = 'installing';
    } catch {
      // The install call reports the failure.
    }
  }

  cancelUpdate() {
    this.update.stage = 'available';
    this.update.progress = null;
  }
}

export const store = new AppStore();
