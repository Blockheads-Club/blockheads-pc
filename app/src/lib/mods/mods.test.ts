import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('../api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import { setLang } from '../i18n.svelte';
import { makeApi, makeWin } from '../mock-api';
import { store } from '../store.svelte';
import { resetMods, resetStore } from '../test-utils';
import { toasts } from '../ui/toast.svelte';
import { makeBrowse, makeBrowseMod, makeMod, makeOverview, makePlan, makePlanItem } from './mock';
import { ago, asError, checkLines, errorReport, errorText, mods, modName, modText, shortDate, sidesOf } from './mods.svelte';
import { permInfo, permKey } from './perms';

const clipboard = vi.fn(async () => {});

beforeEach(async () => {
  mock.api = makeApi();
  mock.win = makeWin().win;
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: clipboard }, configurable: true });
  clipboard.mockClear();
  resetStore();
  await resetMods();
});
afterEach(() => setLang('en'));

describe('mod helpers', () => {
  it('turns anything into a ModError and a text', () => {
    expect(asError({ code: 'io', detail: 'x' })).toEqual({ code: 'io', detail: 'x' });
    expect(asError(new Error('boom'))).toEqual({ code: 'other', detail: 'boom' });
    expect(asError('plain')).toEqual({ code: 'other', detail: 'plain' });
    expect(errorText({ code: 'download', detail: 'dns' })).toBe('The download failed: dns');
    expect(errorText({ code: 'weird' })).toBe('Something went wrong: ');
    expect(errorText({ code: 'insecure' })).toBe('Only https:// links can be installed.');
    expect(errorReport({ code: 'checksum', file: 'a.bhmod', expected: 'e1', got: 'g1', detail: 'd' })).toBe(
      'The file doesn’t match its checksum. It may be broken or changed.\nfile      a.bhmod\nexpected  e1\ngot       g1\ndetail    d'
    );
    expect(errorReport({ code: 'missing' })).toBe('That mod is no longer installed.');
    expect(checkLines({ code: 'size', file: 'f', got: '2' })).toBe('f\ngot       2');
    expect(checkLines({ code: 'checksum', expected: 'e' })).toBe('expected  e');
  });

  it('picks names, texts and sides', () => {
    expect(modName(makeMod())).toBe('Minimap');
    setLang('tr');
    expect(modName(makeMod())).toBe('Mini harita');
    expect(modName(makeMod({ name: { de: 'Karte' } }))).toBe('Karte');
    expect(modName(makeMod({ name: {} }))).toBe('minimap');
    expect(modText({ en: 'a', tr: 'b' })).toBe('b');
    expect(modText({ en: 'a' })).toBe('a');
    expect(modText({ ru: 'c' })).toBe('c');
    expect(modText({})).toBe('');
    expect(sidesOf({ server: true, client: true })).toBe('both');
    expect(sidesOf({ server: true, client: false })).toBe('server');
    expect(sidesOf(['client'])).toBe('client');
    expect(sidesOf(['server', 'client'])).toBe('both');
  });

  it('formats dates', () => {
    expect(shortDate(1_760_227_200)).toBe('Oct 12');
    const now = 1_761_100_000_000;
    expect(ago(now / 1000 - 3600, now)).toBe('today');
    expect(ago(now / 1000 + 100, now)).toBe('today');
    expect(ago(now / 1000 - 14 * 86400, now)).toBe('2 weeks ago');
    expect(ago(now / 1000 - 95 * 86400, now)).toBe('3 months ago');
  });

  it('knows the permission tiers', () => {
    expect(permInfo('net-http:a.test')).toEqual({ key: 'net', tier: 1, icon: 'globe', host: 'a.test' });
    expect(permInfo('native').tier).toBe('native');
    expect(permInfo('surprise')).toEqual({ key: 'other', tier: 1, icon: 'key' });
    expect(permKey(permInfo('channels'), '.desc')).toBe('perm.channels.desc');
  });
});

describe('the mods store', () => {
  it('loads the overview and counts', async () => {
    mock.api.modsOverview = vi.fn(async () => makeOverview());
    await mods.load();
    expect(mods.counts).toEqual({ all: 5, on: 4, problems: 1, updates: 1 });
    expect(mods.current?.id).toBe('minimap');
    mods.filter = 'on';
    expect(mods.shown.map((m) => m.id)).not.toContain('zoom-out');
    mods.filter = 'problems';
    expect(mods.shown.map((m) => m.id)).toEqual(['day-sky']);
    mods.filter = 'updates';
    expect(mods.shown.map((m) => m.id)).toEqual(['better-sounds']);
    mods.filter = 'all';
    mods.query = 'shaders';
    expect(mods.current?.id).toBe('pixel-shaders');
    mods.query = 'nothing like this';
    expect(mods.current).toBeNull();
    mods.query = '';
    mods.select('zoom-out');
    expect([mods.current?.id, mods.showDetail]).toEqual(['zoom-out', true]);
    // A failure keeps the screen and shows the error; quiet reloads do not flash the loading state.
    mock.api.modsOverview = vi.fn(async () => Promise.reject({ code: 'io', detail: 'denied' }));
    await mods.load(true);
    expect(mods.error?.code).toBe('io');
    await resetMods();
    expect(mods.mods).toEqual([]);
  });

  it('opens on a tab and switches tabs', async () => {
    mods.open('browse');
    expect(store.screen).toBe('mods');
    await vi.waitFor(() => expect(mods.browse).not.toBeNull());
    // The first look in a session downloads the list; later ones show what was loaded.
    expect(mock.api.modsIndex).toHaveBeenLastCalledWith(true);
    mods.open('browse');
    expect(mock.api.modsIndex).toHaveBeenLastCalledWith(false);
    mock.api.modsIndex.mockClear();
    mods.open('servers');
    await vi.waitFor(() => expect(mods.servers).not.toBeNull());
    mods.open();
    expect(mods.tab).toBe('servers');
    mods.setTab('browse');
    expect(mock.api.modsIndex).toHaveBeenCalledTimes(0);
    mods.browse = null;
    mods.setTab('browse');
    expect(mock.api.modsIndex).toHaveBeenCalledTimes(1);
    expect(mock.api.modsIndex).toHaveBeenLastCalledWith(true);
    mods.setTab('mine');
    expect(mods.bDetail).toBeNull();
  });

  it('filters the mod list', async () => {
    mods.browse = makeBrowse();
    expect(mods.browsed.map((m) => m.id)).not.toContain('admin');
    mods.bAll = true;
    expect(mods.browsed.map((m) => m.id)).toContain('admin');
    mods.bHideNative = true;
    expect(mods.browsed.map((m) => m.id)).not.toContain('turbo');
    mods.bTag = 'fun';
    expect(mods.browsed.map((m) => m.id)).toEqual(['pets']);
    mods.bTag = null;
    mods.bQuery = 'lumen';
    expect(mods.browsed.map((m) => m.id)).toEqual(['sky-core', 'minimap', 'better-sounds', 'admin', 'keys']);
    mods.browse = null;
    expect(mods.browsed).toEqual([]);
  });

  it('loads the list once with quiet updates, and reports failures', async () => {
    mock.api.modsAutoUpdate = vi.fn(async () => ({ updated: [['a', '1.1.0']], failed: [] }));
    await mods.loadBrowse();
    await vi.waitFor(() => expect(toasts.items.map((x) => x.message)).toContain('1 mod updated'));
    await mods.loadBrowse(true);
    expect(mock.api.modsAutoUpdate).toHaveBeenCalledTimes(1);
    expect(mock.api.modsIndex).toHaveBeenLastCalledWith(true);
    mods.autoUpdated = false;
    store.prefs = { ...store.prefs, mods_auto_update: false };
    await mods.loadBrowse();
    expect(mock.api.modsAutoUpdate).toHaveBeenCalledTimes(1);
    mock.api.modsIndex = vi.fn(async () => Promise.reject({ code: 'offline' }));
    await mods.loadBrowse();
    expect(mods.browseError?.code).toBe('offline');
    mock.api.modsAutoUpdate = vi.fn(async () => Promise.reject(new Error('offline')));
    await mods.autoUpdate();
    mock.api.modsAutoUpdate = vi.fn(async () => ({ updated: [], failed: [] }));
    await mods.autoUpdate();
    expect(mock.api.modsOverview).toHaveBeenCalledTimes(1);
  });

  it('toggles, grants, removes and opens the folder', async () => {
    await mods.load();
    await mods.toggle('minimap', false);
    expect(mock.api.modsSetEnabled).toHaveBeenCalledWith('minimap', false);
    await mods.grant('minimap', 'channels', false);
    expect(mock.api.modsSetGrant).toHaveBeenCalledWith('minimap', 'channels', false);
    mock.api.modsSetEnabled = vi.fn(async () => Promise.reject({ code: 'missing' }));
    await mods.toggle('gone', true);
    expect(toasts.items.at(-1)?.message).toBe('That mod is no longer installed.');
    mods.selected = 'minimap';
    await mods.remove(makeMod());
    expect(mods.selected).toBeNull();
    mods.selected = 'other';
    await mods.remove(makeMod());
    expect(mods.selected).toBe('other');
    await mods.openFolder();
    expect(mock.api.openPath).toHaveBeenCalledWith('/data/mods');
    mods.overview = null;
    mock.api.openPath = vi.fn(async () => Promise.reject('no opener'));
    await mods.openFolder();
    expect(toasts.items.at(-1)?.message).toBe('no opener');
  });

  it('copies text', async () => {
    await mods.copy('x');
    expect(clipboard).toHaveBeenCalledWith('x');
    clipboard.mockRejectedValueOnce(new Error('denied'));
    await mods.copy('y');
    expect(toasts.items.at(-1)?.message).toBe('Could not copy to the clipboard');
  });

  it('lists and clears the server cache', async () => {
    await mods.loadServers();
    expect(mods.servers?.servers).toHaveLength(2);
    await mods.loadServers();
    await mods.clearServer('mira.example.net_15151');
    expect(mock.api.modsClearServer).toHaveBeenCalledWith('mira.example.net_15151');
    await mods.clearServer(null);
    expect(mock.api.modsClearServers).toHaveBeenCalled();
    mock.api.modsClearServers = vi.fn(async () => Promise.reject({ code: 'io', detail: 'busy' }));
    await mods.clearServer(null);
    expect(toasts.items.at(-1)?.message).toBe('The mods folder could not be read or written: busy');
    mock.api.modsServers = vi.fn(async () => Promise.reject({ code: 'io' }));
    await mods.loadServers();
    expect(mods.serversError?.code).toBe('io');
  });

  it('manages developer folders', async () => {
    store.prefs = { ...store.prefs, mods_developer: true, mods_dev_folders: ['/code/a', '/code/b'] };
    mock.api.modsDevInspect = vi.fn(async (p: string) => (p === '/code/b' ? Promise.reject({ code: 'io', detail: 'gone' }) : { ...(await makeApi().modsDevInspect(p)) }));
    await mods.loadDev();
    expect(mods.dev.map((d) => d.errors.length)).toEqual([0, 1]);
    expect(mods.devSelected).toBe('/code/a');
    mods.devSelected = '/code/b';
    await mods.loadDev();
    expect(mods.devSelected).toBe('/code/b');
    // Load a new folder; picking nothing does nothing; a broken folder shows a toast.
    mock.api.pickFolder = vi.fn(async () => null);
    await mods.devAdd();
    mock.api.pickFolder = vi.fn(async () => '/code/c');
    await mods.devAdd();
    expect(store.prefs.mods_dev_folders).toEqual(['/code/a', '/code/b', '/code/c']);
    expect(mods.devSelected).toBe('/code/c');
    expect(mods.devLog.at(-1)).toMatch(/loaded my-mod from its folder$/);
    mock.api.modsDevInspect = vi.fn(async (p: string) => ({ ...(await makeApi().modsDevInspect(p)), id: null }));
    await mods.devAdd();
    expect(store.prefs.mods_dev_folders).toHaveLength(3);
    expect(mods.devLog.at(-1)).toMatch(/loaded \/code\/c from its folder$/);
    mock.api.modsDevInspect = vi.fn(async () => Promise.reject({ code: 'io', detail: 'nope' }));
    await mods.devAdd();
    expect(toasts.items.at(-1)?.message).toMatch(/nope/);
    // Reload: the folder is checked again, then the running game reloads it (or says no game runs).
    mock.api.modsDevInspect = makeApi().modsDevInspect;
    await mods.devReload('/code/a');
    expect(mods.devLog.at(-1)).toMatch(/reloaded my-mod in the running game$/);
    mock.api.modsDevInspect = vi.fn(async (p: string) => ({ ...(await makeApi().modsDevInspect(p)), id: null }));
    await mods.devReload('/code/a');
    expect(mods.devLog.at(-1)).toMatch(/reloaded \/code\/a in the running game$/);
    mock.api.modsDevInspect = makeApi().modsDevInspect;
    mock.api.modsDevReload = vi.fn(async () => Promise.reject({ code: 'notRunning' }));
    await mods.devReload('/code/a');
    expect(mods.devLog.at(-1)).toMatch(/Start the game from this launcher first/);
    await mods.devRemove('/code/a');
    expect(store.prefs.mods_dev_folders).toEqual(['/code/b', '/code/c']);
    await mods.devRemove('/code/b');
    await mods.devRemove('/code/c');
    expect(mods.devSelected).toBeNull();
    mods.devLog = Array.from({ length: 200 }, (_, i) => String(i));
    await mods.devReload('/x');
    expect(mods.devLog).toHaveLength(200);
  });
});

describe('the running game', () => {
  it('shows live states, the game log and joins localhost', async () => {
    const m = makeMod({ id: 'aaa', status: 'ready' });
    expect(mods.liveOf('aaa')).toBeNull();
    expect(mods.statusOf(m)).toBe('ready');
    const live = (state: string, origin = 'mine') => ({ id: 'aaa', version: '1.0.0', state, origin, ms: 1, peak_ms: 2, calls: 3 });
    store.game = { ...store.game, mods: [live('loaded'), live('running', 'server')] };
    expect(mods.liveOf('aaa')?.state).toBe('loaded');
    expect(mods.statusOf(m)).toBe('ready');
    for (const s of ['running', 'budget', 'failed']) {
      store.game = { ...store.game, mods: [live(s)] };
      expect(mods.statusOf(m)).toBe(s);
    }
    store.log = ['[play] x', '[mods] echo: hi'];
    expect(mods.gameLog).toEqual(['echo: hi']);
    await mods.devJoin();
    expect(mock.api.modsDevJoin).toHaveBeenCalledWith('127.0.0.1:15151');
    mock.api.modsDevJoin = vi.fn(async () => Promise.reject({ code: 'io', detail: 'busy' }));
    await mods.devJoin();
    expect(mods.devLog.at(-1)).toMatch(/busy/);
  });

  it('reloads folder mods when their files change', async () => {
    store.prefs = { ...store.prefs, mods_developer: true, mods_dev_folders: ['/code/a'], mods_dev_watch: true };
    await mods.loadDev();
    // No game: nothing is watched.
    await mods.devWatch();
    expect(mock.api.modsDevFingerprint).not.toHaveBeenCalled();
    store.game = { ...store.game, state: 'running' };
    await mods.devWatch();
    await mods.devWatch();
    expect(mock.api.modsDevReload).not.toHaveBeenCalled();
    mock.api.modsDevFingerprint = vi.fn(async () => 'fp2');
    await mods.devWatch();
    expect(mock.api.modsDevReload).toHaveBeenCalledWith('/code/a');
    expect(mods.devLog.some((l) => /files changed/.test(l))).toBe(true);
    store.prefs = { ...store.prefs, mods_dev_watch: false };
    await mods.devWatch();
    expect(mock.api.modsDevFingerprint).toHaveBeenCalledTimes(1);
  });
});

describe('the install flow', () => {
  it('installs from the index with dependencies', async () => {
    await mods.installFromIndex('day-sky');
    expect(mods.install?.stage).toBe('plan');
    expect(mods.install?.grants).toEqual({ 'day-sky': ['world:read'], 'sky-core': [] });
    mods.toggleGrant('day-sky', 'world:read', false);
    mods.toggleGrant('sky-core', 'channels', true);
    mods.toggleGrant('new', 'x', true);
    mods.toggleGrant('new', 'x', true);
    expect(mods.install?.grants).toEqual({ 'day-sky': [], 'sky-core': ['channels'], new: ['x'] });
    const progress = vi.fn();
    mock.api.onModsInstall = vi.fn(async (cb: (p: unknown) => void) => {
      cb({ stage: 'downloading', done: 0, total: 2 });
      return progress;
    });
    mods.browse = makeBrowse();
    await mods.confirm(false);
    expect(mock.api.modsInstall).toHaveBeenCalledWith({
      items: [{ id: 'day-sky', version: '1.0.0' }, { id: 'sky-core', version: '1.2.0' }], token: null,
      grants: { 'day-sky': [], 'sky-core': ['channels'], new: ['x'] }, enable: false, native_confirmed: false
    });
    expect(mods.install?.stage).toBe('done');
    expect(progress).toHaveBeenCalled();
    mods.showInstalled();
    expect([mods.install, mods.selected, mods.tab]).toEqual([null, 'day-sky', 'mine']);
  });

  it('goes straight to the permissions for one mod and updates', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem()] }));
    await mods.update(makeMod({ update: '1.4.0' }));
    expect(mock.api.modsPlan).toHaveBeenCalledWith('minimap', '1.4.0');
    expect(mods.install?.stage).toBe('perms');
    mods.close();
    expect(mods.install).toBeNull();
  });

  it('asks for the native confirmation and installs a checked file', async () => {
    mock.api.modsInspectFile = vi.fn(async () => makePlan({ items: [makePlanItem({ id: 'ps', native: true, permissions: ['native'] }), makePlanItem({ id: 'dep', needed_by: 'ps' })], token: 'tok' }));
    mock.api.onModsInstall = vi.fn(async () => Promise.reject(new Error('no events')));
    await mods.inspectFile('/a.bhmod');
    expect(mods.install?.grants.ps).toEqual([]);
    await mods.confirm(true);
    expect(mods.install?.stage).toBe('native');
    await mods.commit(true);
    expect(mock.api.modsInstall).toHaveBeenCalledWith(expect.objectContaining({ items: [{ id: 'dep', version: '1.0.0' }], token: 'tok', native_confirmed: true, enable: true }));
  });

  it('picks a file, checks links and offers a retry', async () => {
    mods.startFile();
    expect(mods.install?.stage).toBe('source');
    mock.api.pickFile = vi.fn(async () => null);
    await mods.pickFile();
    expect(mods.install?.stage).toBe('source');
    mock.api.pickFile = vi.fn(async () => '/m.bhmod');
    await mods.pickFile();
    expect(mock.api.modsInspectFile).toHaveBeenCalledWith('/m.bhmod');
    mods.startFile();
    mods.install!.link = ' https://e.test/m.bhmod ';
    mock.api.modsInspectLink = vi.fn(async () => Promise.reject({ code: 'checksum', expected: 'a', got: 'b' }));
    await mods.inspectLink();
    expect(mods.install?.stage).toBe('failed');
    expect(mock.api.modsInspectLink).toHaveBeenCalledWith('https://e.test/m.bhmod');
    mock.api.modsInspectLink = makeApi().modsInspectLink;
    await mods.install!.retry!();
    expect(mods.install?.stage).toBe('perms');
    // A file check failure can be retried too.
    mock.api.modsInspectFile = vi.fn(async () => Promise.reject({ code: 'invalid', detail: 'no mod.toml' }));
    await mods.inspectFile('/b.zip');
    expect(mods.install?.error?.code).toBe('invalid');
    await mods.install!.retry!();
    expect(mock.api.modsInspectFile).toHaveBeenCalledTimes(2);
  });

  it('shows the newer-loader choice and plan failures', async () => {
    mock.api.modsPlan = vi.fn(async () => Promise.reject({ code: 'newerLoader', file: 'Key Hints 2.0.0', expected: '^1.4', detail: '1.6.0' }));
    await mods.installFromIndex('keys');
    expect(mods.install?.newer).toEqual({ id: 'keys', name: 'Key Hints 2.0.0', needs: '^1.4', alt: '1.6.0' });
    mock.api.modsPlan = vi.fn(async () => Promise.reject({ code: 'newerLoader' }));
    await mods.installFromIndex('keys');
    expect(mods.install?.newer).toEqual({ id: 'keys', name: '', needs: '', alt: null });
    // A newer-loader answer for a file has no index id.
    mock.api.modsInspectFile = vi.fn(async () => Promise.reject({ code: 'newerLoader' }));
    mods.install = null as typeof mods.install;
    await mods.inspectFile('/x.bhmod');
    expect(mods.install!.newer?.id).toBe('');
    mock.api.modsPlan = vi.fn(async () => Promise.reject({ code: 'notListed', detail: 'keys' }));
    await mods.installFromIndex('keys');
    expect((mods.install as { stage: string } | null)?.stage).toBe('failed');
    await mods.install!.retry!();
    expect(mock.api.modsPlan).toHaveBeenCalledTimes(2);
  });

  it('keeps the dialog while installing and can retry a failed install', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem()] }));
    await mods.installFromIndex('day-sky');
    let finish!: () => void;
    mock.api.modsInstall = vi.fn(() => new Promise<string[]>((r) => (finish = () => r(['day-sky']))));
    const p = mods.confirm(true);
    await vi.waitFor(() => expect(mock.api.modsInstall).toHaveBeenCalled());
    expect(mods.install?.stage).toBe('installing');
    mods.close();
    expect(mods.install).not.toBeNull();
    finish();
    await p;
    mock.api.modsInstall = vi.fn(async () => Promise.reject({ code: 'size', expected: '3', got: '2' }));
    await mods.commit(false);
    expect(mods.install?.stage).toBe('failed');
    mock.api.modsInstall = vi.fn(async () => ['day-sky']);
    await mods.install!.retry!();
    expect(mods.install?.stage).toBe('done');
  });

  it('uses the browse data after an install only when it was loaded', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem()] }));
    await mods.installFromIndex('day-sky');
    await mods.commit(false);
    expect(mock.api.modsIndex).not.toHaveBeenCalled();
    expect(makeBrowseMod().id).toBe('sky-core');
  });
});
