import { api } from '../api';
import { i18n, t } from '../i18n.svelte';
import type { Key } from '../messages';
import { store } from '../store.svelte';
import { toast } from '../ui/toast.svelte';
import type {
  Browse, BrowseMod, CacheOverview, DevMod, InstallProgress, LiveMod, ModError, ModStatus, ModSummary, Overview, Plan
} from './types';

export type ModsTab = 'mine' | 'browse' | 'servers' | 'dev';
export type MineFilter = 'all' | 'on' | 'problems' | 'updates';
export type InstallStage = 'source' | 'checking' | 'plan' | 'perms' | 'native' | 'installing' | 'done' | 'failed' | 'newer';

/** Statuses that need the player's attention (the count on Home). */
export const PROBLEMS = ['failed', 'blocked', 'dependency', 'permissions', 'budget'];

/** The install dialog. */
export interface InstallState {
  stage: InstallStage;
  /** Empty until the checks are done. */
  plan: Plan;
  error: ModError | null;
  /** Granted permissions per mod (ticked boxes); native is decided by the confirm step. */
  grants: Record<string, string[]>;
  /** The two native ticks. */
  confirm: [boolean, boolean];
  enable: boolean;
  progress: InstallProgress | null;
  link: string;
  /** What "Download again" / "Try again" repeats. */
  retry: (() => Promise<void>) | null;
  /** `newerLoader`: the mod, what it needs and the version that works here. */
  newer: { id: string; name: string; needs: string; alt: string | null } | null;
}

/** A ModError from anything a command rejected with. */
export function asError(e: unknown): ModError {
  if (e && typeof e === 'object' && 'code' in e) return e as ModError;
  return { code: 'other', detail: e instanceof Error ? e.message : String(e) };
}

/** The text of a mod error in the UI language. */
export function errorText(e: ModError): string {
  const known = ['io', 'config', 'missing', 'permission', 'download', 'insecure', 'tooLarge', 'index', 'offline', 'notListed', 'serverOnly',
    'newerLoader', 'invalid', 'size', 'checksum', 'mismatch', 'nativeConfirm', 'nativeBlocked', 'expired', 'needsLoader', 'notRunning'];
  const key = (known.includes(e.code) ? `mods.err.${e.code}` : 'mods.err.other') as Key;
  return t(key, { detail: e.detail ?? '', file: e.file ?? '', expected: e.expected ?? '', got: e.got ?? '' });
}

/** Details for "Copy error": everything the error carries. */
export function errorReport(e: ModError): string {
  return [errorText(e), e.file && `file      ${e.file}`, e.expected && `expected  ${e.expected}`, e.got && `got       ${e.got}`, e.detail && `detail    ${e.detail}`]
    .filter(Boolean)
    .join('\n');
}

/** The lines of a failed file check: the file, what was expected and what came. */
export function checkLines(e: ModError): string {
  return [e.file, e.expected && `${t('mods.inst.expected')}  ${e.expected}`, e.got && `${t('mods.inst.got')}       ${e.got}`].filter(Boolean).join('\n');
}

/** The mod's name in the UI language (English, then any, then the id). */
export function modName(m: { id: string; name: Record<string, string> }): string {
  return m.name[i18n.lang] ?? m.name.en ?? Object.values(m.name)[0] ?? m.id;
}

export function modText(map: Record<string, string>): string {
  return map[i18n.lang] ?? map.en ?? Object.values(map)[0] ?? '';
}

export function sidesOf(s: { server: boolean; client: boolean } | string[]): 'server' | 'client' | 'both' {
  const server = Array.isArray(s) ? s.includes('server') : s.server;
  const client = Array.isArray(s) ? s.includes('client') : s.client;
  return server && client ? 'both' : server ? 'server' : 'client';
}

/** A date from unix seconds, short and in the UI language. */
export function shortDate(secs: number): string {
  return new Intl.DateTimeFormat(i18n.lang, { day: 'numeric', month: 'short' }).format(new Date(secs * 1000));
}

/** "today", "2 weeks ago"… from unix seconds. */
export function ago(secs: number, now = Date.now()): string {
  const days = Math.floor((now / 1000 - secs) / 86400);
  const rtf = new Intl.RelativeTimeFormat(i18n.lang, { numeric: 'auto' });
  if (days < 7) return rtf.format(-Math.max(days, 0), 'day');
  if (days < 60) return rtf.format(-Math.floor(days / 7), 'week');
  return rtf.format(-Math.floor(days / 30), 'month');
}

function matches(q: string, ...texts: (string | null | undefined)[]): boolean {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  const hay = texts.filter(Boolean).join(' ').toLowerCase();
  return words.every((w) => hay.includes(w));
}

/** State and actions of the Mods area; one instance. */
export class ModsStore {
  tab = $state<ModsTab>('mine');
  overview = $state<Overview | null>(null);
  loading = $state(false);
  /** The overview command failed (the folder cannot be read). */
  error = $state<ModError | null>(null);
  selected = $state<string | null>(null);
  /** Narrow window: the details pane replaces the list. */
  showDetail = $state(false);
  query = $state('');
  filter = $state<MineFilter>('all');
  busy = $state<string | null>(null);

  browse = $state<Browse | null>(null);
  browseLoading = $state(false);
  browseError = $state<ModError | null>(null);
  bQuery = $state('');
  bAll = $state(false);
  bHideNative = $state(false);
  bTag = $state<string | null>(null);
  bDetail = $state<string | null>(null);

  servers = $state<CacheOverview | null>(null);
  serversLoading = $state(false);
  serversError = $state<ModError | null>(null);

  dev = $state<DevMod[]>([]);
  devSelected = $state<string | null>(null);
  devLog = $state<string[]>([]);

  install = $state<InstallState | null>(null);
  /** Quiet updates ran this session. */
  autoUpdated = false;

  get mods(): ModSummary[] {
    return this.overview?.mods ?? [];
  }

  get counts() {
    const m = this.mods;
    return {
      all: m.length,
      on: m.filter((x) => x.enabled).length,
      problems: m.filter((x) => PROBLEMS.includes(x.status)).length,
      updates: m.filter((x) => x.update).length
    };
  }

  /** My mods after search and filter. */
  get shown(): ModSummary[] {
    return this.mods.filter((m) => {
      if (this.filter === 'on' && !m.enabled) return false;
      if (this.filter === 'problems' && !PROBLEMS.includes(m.status)) return false;
      if (this.filter === 'updates' && !m.update) return false;
      return matches(this.query, modName(m), m.id, ...m.authors);
    });
  }

  get current(): ModSummary | null {
    const list = this.shown;
    return list.find((m) => m.id === this.selected) ?? list[0] ?? null;
  }

  /** The mod list after search, sides, native and tag filters. */
  get browsed(): BrowseMod[] {
    return (this.browse?.mods ?? []).filter((m) => {
      if (!this.bAll && !m.sides.includes('client')) return false;
      if (this.bHideNative && m.native) return false;
      if (this.bTag && !m.tags.includes(this.bTag)) return false;
      return matches(this.bQuery, m.name, m.description, m.id, ...m.owners, ...m.tags);
    });
  }

  open(tab: ModsTab = this.tab) {
    this.tab = tab;
    store.go('mods');
    void this.load();
    if (tab === 'browse') void this.loadBrowse(!this.browse);
    if (tab === 'servers') void this.loadServers();
  }

  setTab(tab: ModsTab) {
    this.tab = tab;
    this.showDetail = false;
    this.bDetail = null;
    if (tab === 'browse' && !this.browse) void this.loadBrowse(true);
    if (tab === 'servers') void this.loadServers();
  }

  select(id: string) {
    this.selected = id;
    this.showDetail = true;
  }

  async load(quiet = false) {
    if (!quiet) this.loading = !this.overview;
    try {
      this.overview = await api.modsOverview();
      this.error = null;
    } catch (e) {
      this.error = asError(e);
    } finally {
      this.loading = false;
    }
  }

  /** The public list: downloaded the first time it is shown in a session (the saved copy when offline), so a stale
      copy never stands for it; quiet updates follow when the setting is on. */
  async loadBrowse(refresh = false) {
    this.browseLoading = true;
    try {
      this.browse = await api.modsIndex(refresh);
      this.browseError = null;
      if (store.prefs.mods_auto_update && !this.autoUpdated) {
        this.autoUpdated = true;
        void this.autoUpdate();
      }
    } catch (e) {
      this.browseError = asError(e);
    } finally {
      this.browseLoading = false;
    }
  }

  async autoUpdate() {
    try {
      const r = await api.modsAutoUpdate();
      if (r.updated.length) {
        toast(t('mods.autoUpdated', { n: r.updated.length }), undefined, 'ok');
        await this.load(true);
      }
    } catch {
      // Offline: the updates wait for the next time.
    }
  }

  private async act(id: string, f: () => Promise<unknown>) {
    this.busy = id;
    try {
      await f();
    } catch (e) {
      toast(errorText(asError(e)), undefined, 'bad');
    } finally {
      this.busy = null;
      await this.load(true);
    }
  }

  toggle(id: string, on: boolean) {
    return this.act(id, () => api.modsSetEnabled(id, on));
  }

  grant(id: string, perm: string, on: boolean) {
    return this.act(id, () => api.modsSetGrant(id, perm, on));
  }

  async remove(m: ModSummary) {
    await this.act(m.id, () => api.modsRemove(m.id));
    if (this.selected === m.id) this.selected = null;
    this.showDetail = false;
    toast(t('mods.removed', { name: modName(m) }));
  }

  async openFolder() {
    try {
      await api.openPath(this.overview?.dir ?? '');
    } catch (e) {
      store.fail(e);
    }
  }

  async copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      toast(t('mods.copied'), undefined, 'ok');
    } catch {
      toast(t('common.copyFailed'), undefined, 'bad');
    }
  }

  async loadServers() {
    this.serversLoading = !this.servers;
    try {
      this.servers = await api.modsServers();
      this.serversError = null;
    } catch (e) {
      this.serversError = asError(e);
    } finally {
      this.serversLoading = false;
    }
  }

  async clearServer(key: string | null) {
    try {
      this.servers = key ? await api.modsClearServer(key) : await api.modsClearServers();
      toast(t('mods.srv.cleared'), undefined, 'ok');
    } catch (e) {
      toast(errorText(asError(e)), undefined, 'bad');
    }
  }

  // ---- developer mode ----------------------------------------------------------------------------------------

  async loadDev() {
    this.dev = await Promise.all(
      store.prefs.mods_dev_folders.map((p) => api.modsDevInspect(p).catch((e) => ({
        path: p, id: null, version: null, name: null, permissions: [], native: false, errors: [errorText(asError(e))], warnings: []
      })))
    );
    this.devSelected ??= this.dev[0]?.path ?? null;
  }

  async devAdd() {
    const path = await api.pickFolder();
    if (!path) return;
    try {
      const m = await api.modsDevInspect(path);
      if (!store.prefs.mods_dev_folders.includes(path)) await store.patchPrefs({ mods_dev_folders: [...store.prefs.mods_dev_folders, path] });
      this.dev = [...this.dev.filter((d) => d.path !== path), m];
      this.devSelected = path;
      this.log(t('mods.dev.logLoaded', { name: m.id ?? path }));
    } catch (e) {
      toast(errorText(asError(e)), undefined, 'bad');
    }
  }

  async devRemove(path: string) {
    await store.patchPrefs({ mods_dev_folders: store.prefs.mods_dev_folders.filter((p) => p !== path) });
    this.dev = this.dev.filter((d) => d.path !== path);
    this.devSelected = this.dev[0]?.path ?? null;
  }

  async devReload(path: string) {
    try {
      const m = await api.modsDevInspect(path);
      this.dev = this.dev.map((d) => (d.path === path ? m : d));
      await api.modsDevReload(path);
      this.log(t('mods.dev.reloaded', { name: m.id ?? path }));
    } catch (e) {
      this.log(errorText(asError(e)));
    }
  }

  private devFp: string | null = null;

  /** Live reload, called every second while the Developer tab shows: reloads when a folder's files changed. */
  async devWatch() {
    if (!store.prefs.mods_dev_watch || !store.active || !this.devSelected) {
      this.devFp = null;
      return;
    }
    const fp = await api.modsDevFingerprint(store.prefs.mods_dev_folders);
    const changed = this.devFp !== null && fp !== this.devFp;
    this.devFp = fp;
    if (changed) {
      this.log(t('mods.dev.changedLog'));
      await this.devReload(this.devSelected);
    }
  }

  /** Starts the game and joins the Blockheads Server on this computer. */
  async devJoin() {
    try {
      await api.modsDevJoin('127.0.0.1:15151');
    } catch (e) {
      this.log(errorText(asError(e)));
    }
  }

  /** The running game's report for an installed mod (its own copy, not a server's). */
  liveOf(id: string): LiveMod | null {
    return store.game.mods?.find((l) => l.id === id && l.origin === 'mine') ?? null;
  }

  /** The status to show: while the game runs a mod, its live state (running, turned off for its budget, failed). */
  statusOf(m: ModSummary): ModStatus {
    const l = this.liveOf(m.id);
    return l && (l.state === 'running' || l.state === 'budget' || l.state === 'failed') ? l.state : m.status;
  }

  /** The game's mod-loader lines (its log's `[mods]` lines). */
  get gameLog(): string[] {
    return store.log.filter((l) => l.startsWith('[mods] ')).map((l) => l.slice(7));
  }

  private log(line: string) {
    const ts = new Date().toLocaleTimeString(i18n.lang, { hour12: false });
    this.devLog = [...this.devLog.slice(-199), `${ts}  ${line}`];
  }

  // ---- install -----------------------------------------------------------------------------------------------

  private begin(over: Partial<InstallState>) {
    this.install = {
      stage: 'checking', plan: { items: [], satisfied: [], missing: [], total_bytes: 0, token: null }, error: null, grants: {}, confirm: [false, false], enable: true, progress: null, link: '', retry: null,
      newer: null, ...over
    };
  }

  /** The plan is in: show what gets installed, or go straight to the permissions when it is one mod. */
  private planned(plan: Plan) {
    const grants: Record<string, string[]> = {};
    for (const it of plan.items) grants[it.id] = it.permissions.filter((p) => p !== 'native');
    const simple = plan.items.length === 1 && !plan.missing.length;
    Object.assign(this.install!, { plan, grants, stage: simple ? 'perms' : 'plan', error: null });
  }

  private failed(e: unknown, retry: (() => Promise<void>) | null) {
    const err = asError(e);
    if (err.code === 'newerLoader') {
      Object.assign(this.install!, { stage: 'newer', newer: { id: this.install!.newer?.id ?? '', name: err.file ?? '', needs: err.expected ?? '', alt: err.detail ?? null } });
      return;
    }
    Object.assign(this.install!, { stage: 'failed', error: err, retry });
  }

  /** Install a mod from the public list (with its dependencies). */
  async installFromIndex(id: string, version: string | null = null) {
    this.begin({ newer: { id, name: '', needs: '', alt: null } });
    const run = async () => {
      this.install!.stage = 'checking';
      try {
        this.planned(await api.modsPlan(id, version));
      } catch (e) {
        this.failed(e, run);
      }
    };
    await run();
  }

  /** Update an installed mod to the newest version on the list. */
  update(m: ModSummary) {
    return this.installFromIndex(m.id, m.update);
  }

  startFile() {
    this.begin({ stage: 'source' });
  }

  async inspectFile(path: string) {
    if (!this.install) this.begin({});
    const run = async () => {
      this.install!.stage = 'checking';
      try {
        this.planned(await api.modsInspectFile(path));
      } catch (e) {
        this.failed(e, run);
      }
    };
    await run();
  }

  async pickFile() {
    const path = await api.pickFile(['bhmod', 'zip'], t('mods.inst.fileType'));
    if (path) await this.inspectFile(path);
  }

  async inspectLink() {
    const url = this.install!.link.trim();
    const run = async () => {
      this.install!.stage = 'checking';
      try {
        this.planned(await api.modsInspectLink(url));
      } catch (e) {
        this.failed(e, run);
      }
    };
    await run();
  }

  toggleGrant(id: string, perm: string, on: boolean) {
    const g = this.install!.grants[id] ?? [];
    this.install!.grants[id] = on ? [...g.filter((p) => p !== perm), perm] : g.filter((p) => p !== perm);
  }

  /** "Install and turn on" / "Install, keep off": native packages need the confirm step first. */
  async confirm(enable: boolean) {
    const ins = this.install!;
    ins.enable = enable;
    if (ins.plan.items.some((i) => i.native)) {
      ins.confirm = [false, false];
      ins.stage = 'native';
      return;
    }
    await this.commit(false);
  }

  async commit(native: boolean) {
    const ins = this.install!;
    const plan = ins.plan;
    const token = plan.token;
    const items = plan.items.filter((i, n) => !(token && n === 0)).map((i) => ({ id: i.id, version: i.version }));
    const run = async () => {
      ins.stage = 'installing';
      ins.progress = null;
      const off = await api.onModsInstall((p) => (ins.progress = p)).catch(() => undefined);
      try {
        await api.modsInstall({ items, token, grants: ins.grants, enable: ins.enable, native_confirmed: native });
        ins.stage = 'done';
        await this.load(true);
        if (this.browse) void this.loadBrowse();
      } catch (e) {
        this.failed(e, run);
      } finally {
        off?.();
      }
    };
    await run();
  }

  close() {
    if (this.install?.stage !== 'installing') this.install = null;
  }

  /** "Open in My mods" after an install. */
  showInstalled() {
    const id = this.install!.plan.items[0].id;
    this.install = null;
    this.selected = id;
    this.setTab('mine');
  }
}

export const mods = new ModsStore();
