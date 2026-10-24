import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('../api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import { makeApi, makeWin } from '../mock-api';
import { store } from '../store.svelte';
import { resetMods, resetStore } from '../test-utils';
import { viewport } from '../viewport.svelte';
import { makeBrowse, makeBrowseMod, makeDevMod, makeMod, makeOverview, makePlan, makePlanItem, makeServers } from './mock';
import { mods } from './mods.svelte';
import InstallFlow from './InstallFlow.svelte';
import ModDetail from './ModDetail.svelte';
import ModsBrowse from './ModsBrowse.svelte';
import ModsDev from './ModsDev.svelte';
import ModsScreen from './ModsScreen.svelte';
import ModsServers from './ModsServers.svelte';
import ModStatus from './ModStatus.svelte';
import MyMods from './MyMods.svelte';
import Badges from './Badges.svelte';
import PermissionChip from './PermissionChip.svelte';
import SettingsMods from '../screens/SettingsMods.svelte';

const click = (name: string | RegExp) => fireEvent.click(screen.getByRole('button', { name }));
const clipboard = vi.fn(async () => {});
let hooks: ReturnType<typeof makeWin>['hooks'];

beforeEach(async () => {
  mock.api = makeApi();
  const w = makeWin();
  mock.win = w.win;
  hooks = w.hooks;
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: clipboard }, configurable: true });
  resetStore({ screen: 'mods' });
  await resetMods();
  viewport.modsNarrow = false;
});
afterEach(() => cleanup());

describe('Mods screen', () => {
  it('shows tabs and switches tabs with the keyboard', async () => {
    render(ModsScreen);
    await screen.findByRole('button', { name: /Minimap/ });
    const mine = screen.getByRole('tab', { name: /My mods/ });
    expect(mine).toHaveTextContent('5');
    expect(screen.queryByRole('tab', { name: 'Developer' })).toBeNull();
    await fireEvent.keyDown(mine, { key: 'ArrowRight' });
    expect(mods.tab).toBe('browse');
    await fireEvent.keyDown(screen.getByRole('tab', { name: 'Mod list' }), { key: 'ArrowLeft' });
    expect(mods.tab).toBe('mine');
    await fireEvent.keyDown(mine, { key: 'ArrowLeft' });
    expect(mods.tab).toBe('servers');
    await fireEvent.keyDown(mine, { key: 'Enter' });
    await click(/Install from file or link/);
    expect(mods.install?.stage).toBe('source');
    await click('Home');
    expect(store.screen).toBe('home');
  });

  it('shows the developer tab and uses Esc to go back', async () => {
    store.prefs = { ...store.prefs, mods_developer: true };
    viewport.modsNarrow = true;
    render(ModsScreen);
    await fireEvent.click(screen.getByRole('tab', { name: 'Developer' }));
    expect(screen.queryByRole('button', { name: /Install from file/ })).toBeNull();
    await fireEvent.click(screen.getByRole('tab', { name: /My mods/ }));
    expect(screen.getByRole('button', { name: 'Install from file or link' })).toBeVisible();
    // Esc: a detail view first, then home; not while typing or with a dialog open.
    mods.bDetail = 'x';
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(mods.bDetail).toBeNull();
    mods.showDetail = true;
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(mods.showDetail).toBe(false);
    const input = document.createElement('input');
    input.value = 'x';
    document.body.append(input);
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(store.screen).toBe('mods');
    input.value = '';
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(store.screen).toBe('home');
    store.screen = 'mods';
    mods.startFile();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect([mods.install, store.screen]).toEqual([null, 'mods']);
    await fireEvent.keyDown(window, { key: 'a' });
    store.dialog = 'join';
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(store.screen).toBe('mods');
    input.remove();
  });

  it('counts servers on their tab', async () => {
    mods.servers = makeServers();
    render(ModsScreen);
    expect(screen.getByRole('tab', { name: /Server mods/ })).toHaveTextContent('2');
    mods.tab = 'servers';
    await screen.findByText('Sunny Isle');
  });
});

describe('My mods', () => {
  it('lists, filters, selects and toggles', async () => {
    await mods.load();
    render(MyMods);
    await click('Problems 1');
    expect(within(screen.getByRole('list', { name: 'My mods' })).getAllByRole('listitem')).toHaveLength(1);
    await click('All 5');
    const zoom = screen.getByRole('button', { name: /Zoom Out/ });
    await fireEvent.click(zoom);
    expect(mods.selected).toBe('zoom-out');
    expect(screen.getByRole('heading', { name: /Zoom Out/ })).toBeVisible();
    // Arrow keys move the selection; other keys and the ends do nothing.
    await fireEvent.keyDown(screen.getByRole('button', { name: /Zoom Out/ }), { key: 'ArrowDown' });
    expect(mods.selected).toBe('day-sky');
    await fireEvent.keyDown(screen.getByRole('button', { name: /Day & Night/ }), { key: 'ArrowUp' });
    expect(mods.selected).toBe('zoom-out');
    await fireEvent.keyDown(screen.getByRole('button', { name: /Zoom Out/ }), { key: 'x' });
    await fireEvent.keyDown(screen.getAllByRole('list')[0], { key: 'ArrowDown' });
    expect(mods.selected).toBe('zoom-out');
    await fireEvent.click(screen.getByRole('switch', { name: 'Zoom Out on or off' }));
    expect(mock.api.modsSetEnabled).toHaveBeenCalledWith('zoom-out', true);
    await fireEvent.input(screen.getByPlaceholderText('Search my mods'), { target: { value: 'zzz' } });
    expect(screen.getByText('No mod matches.')).toBeVisible();
  });

  it('shows loading, empty, error and broken-file states', async () => {
    const { unmount } = render(MyMods);
    expect(screen.getByRole('status')).toHaveTextContent('Loading your mods…');
    unmount();
    mods.overview = makeOverview({ mods: [] });
    const r2 = render(MyMods);
    expect(screen.getByRole('heading', { name: 'No mods yet' })).toBeVisible();
    await click('Browse the mod list');
    expect(mods.tab).toBe('browse');
    await click('Install from a file or link');
    expect(mods.install?.stage).toBe('source');
    r2.unmount();
    mods.overview = makeOverview({ mods: [], problems: [{ file: 'bad.bhmod', message: 'invalid: x' }] });
    const r3 = render(MyMods);
    expect(screen.getByText('bad.bhmod: invalid: x')).toBeVisible();
    r3.unmount();
    mods.overview = null;
    mods.error = { code: 'io', detail: 'denied', file: '/data/mods' };
    mock.api.modsOverview = vi.fn(async () => Promise.reject({ code: 'io', file: '/data/mods' }));
    const r4 = render(MyMods);
    expect(screen.getByRole('alert')).toHaveTextContent('/data/mods');
    await click('Try again');
    expect(mock.api.modsOverview).toHaveBeenCalled();
    mods.error = { code: 'io' };
    await click('Copy error');
    expect(clipboard).toHaveBeenCalled();
    await click('Open folder');
    expect(mock.api.openPath).toHaveBeenCalledWith('');
    r4.unmount();
    mods.overview = makeOverview();
    mods.error = { code: 'config' };
    render(MyMods);
    expect(screen.getByRole('alert')).toHaveTextContent('/data/mods');
  });

  it('shows one pane in a narrow window', async () => {
    await mods.load();
    render(MyMods, { narrow: true });
    expect(screen.queryByRole('main')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: /Better Sounds/ }));
    expect(screen.queryByPlaceholderText('Search my mods')).toBeNull();
    await click('My mods');
    expect(mods.showDetail).toBe(false);
  });
});

describe('Mod details', () => {
  it('shows a ready mod with its permissions and actions', async () => {
    const m = makeMod({ granted: ['world:read'], missing: ['channels'] });
    render(ModDetail, { m });
    expect(screen.getByText('by pixelpan · From the mod list · Oct 3')).toBeVisible();
    await click('Revoke');
    expect(mock.api.modsSetGrant).toHaveBeenCalledWith('minimap', 'world:read', false);
    await click('Allow');
    expect(mock.api.modsSetGrant).toHaveBeenCalledWith('minimap', 'channels', true);
    await fireEvent.click(screen.getByRole('switch', { name: 'On' }));
    expect(mock.api.modsSetEnabled).toHaveBeenCalledWith('minimap', false);
    await click('github.com/pixelpan/minimap');
    expect(mock.api.openUrl).toHaveBeenCalledWith('https://github.com/pixelpan/minimap');
    await click('Open folder');
    await click('Remove');
    await click('Cancel');
    await click('Remove');
    await fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(mock.api.modsRemove).toHaveBeenCalledWith('minimap'));
  });

  it('explains every status', async () => {
    store.game = { ...store.game, mods: [{ id: makeMod().id, version: '1', state: 'budget', origin: 'mine', ms: 0.5, peak_ms: 30, calls: 2, reason: '3 faults within 60 s' }] };
    const r = render(ModDetail, { m: makeMod({ status: 'failed', reason: 'server-only', source: 'link', update: '1.0.0', update_asks_more: true }) });
    expect(screen.getByText(/0.5 ms of mod code per second · longest call 30.0 ms/)).toBeVisible();
    expect(screen.getByText('3 faults within 60 s')).toBeVisible();
    await r.rerender({ m: makeMod({ status: 'failed', reason: 'server-only', source: 'link' }) });
    store.game = { ...store.game, mods: [{ id: makeMod().id, version: '1', state: 'running', origin: 'mine', ms: 0, peak_ms: 0, calls: 1 }] };
    await r.rerender({ m: makeMod({ status: 'ready' }) });
    expect(screen.queryByText('3 faults within 60 s')).toBeNull();
    store.game = { ...store.game, mods: null };
    await r.rerender({ m: makeMod({ status: 'failed', reason: 'server-only', source: 'link', update: '1.0.0', update_asks_more: true }) });
    expect(screen.getByText('This mod has no part for players. It belongs on a server.')).toBeVisible();
    expect(screen.getByText('It asks for new permissions, so it waits for you.')).toBeVisible();
    await click('Update 1.0.0');
    expect(mock.api.modsPlan).toHaveBeenCalledWith('minimap', '1.0.0');
    await r.rerender({ m: makeMod({ status: 'failed', reason: 'loader', detail: '^9', source: 'file', authors: [], installed_at: 0 }) });
    expect(screen.getByText(/newer mod loader \(\^9\)/)).toBeVisible();
    await r.rerender({ m: makeMod({ status: 'failed', reason: 'loader', detail: null, source: '' }) });
    await r.rerender({ m: makeMod({ status: 'blocked', native: true, permissions: ['native'], granted: [], reviewed: false, native_allowed_at: null }) });
    expect(screen.getByText('Native mods are blocked')).toBeVisible();
    expect(screen.getByText(/only given when you install/)).toBeVisible();
    await click('Open Settings');
    expect(store.section).toBe('mods');
    await r.rerender({ m: makeMod({ status: 'dependency', dependencies: [
      { id: 'sky-core', req: '^1', installed: null, ok: false, soft: false, available: '1.2.0' },
      { id: 'soft', req: '^1', installed: null, ok: false, soft: true, available: null },
      { id: 'old', req: '^2', installed: '1.0.0', ok: false, soft: false, available: null },
      { id: 'fine', req: '^1', installed: '1.0.0', ok: true, soft: false, available: null }
    ] }) });
    expect(screen.getByText('Waiting for sky-core')).toBeVisible();
    expect(screen.getByText('Wrong version 1.0.0')).toBeVisible();
    await click('Install sky-core 1.2.0');
    expect(mock.api.modsPlan).toHaveBeenCalledWith('sky-core', '1.2.0');
    await r.rerender({ m: makeMod({ status: 'dependency', dependencies: [{ id: 'x', req: '^1', installed: null, ok: false, soft: false, available: null }] }) });
    expect(screen.queryByRole('button', { name: /Install x/ })).toBeNull();
    await r.rerender({ m: makeMod({ status: 'dependency', dependencies: [] }) });
    await r.rerender({ m: makeMod({ status: 'permissions', missing: ['channels'] }) });
    expect(screen.getByText('Needs your permission')).toBeVisible();
    await r.rerender({ m: makeMod({ native: true, permissions: ['native', 'net-http:a.test'], granted: ['native'], native_allowed_at: 1_760_227_200, homepage: null, license: '', description: {} }) });
    expect(screen.getByText(/You allowed this on Oct 12/)).toBeVisible();
    expect(screen.getByText('Allowed by you · Oct 12')).toBeVisible();
    expect(screen.getByText('Internet: a.test')).toBeVisible();
    await click('Revoke (turns it off)');
    expect(mock.api.modsSetGrant).toHaveBeenCalledWith('minimap', 'native', false);
    await r.rerender({ m: makeMod({ native: true, permissions: ['native'], granted: ['native'], native_allowed_at: null }) });
    expect(screen.getByText('Native mods have no limits, so there is nothing to measure.')).toBeVisible();
    await r.rerender({ m: makeMod({ permissions: [], granted: [] }) });
    expect(screen.getByText('Only changes how the game looks and sounds on this computer.')).toBeVisible();
  });
});

describe('Mod list', () => {
  it('shows cards, filters and the details page', async () => {
    mods.browse = makeBrowse();
    render(ModsBrowse);
    expect(document.body).toHaveTextContent('6 of 7 mods');
    expect(screen.getAllByText('Installed')).toHaveLength(1);
    await click('Install Sky Core');
    expect(mock.api.modsPlan).toHaveBeenCalledWith('sky-core', null);
    await click('Update');
    await fireEvent.click(within(screen.getByRole('group', { name: 'Sides' })).getByRole('button', { name: 'All' }));
    expect(document.body).toHaveTextContent('7 of 7 mods');
    expect(screen.getByText(/installs on a Blockheads Server/)).toBeVisible();
    await click('For players');
    await fireEvent.click(screen.getByRole('switch', { name: 'Hide native' }));
    expect(mods.bHideNative).toBe(true);
    await click('fun');
    expect(mods.bTag).toBe('fun');
    await fireEvent.click(screen.getAllByRole('button', { name: 'All' })[1]);
    expect(mods.bTag).toBeNull();
    await fireEvent.input(screen.getByPlaceholderText('Search mods, authors, tags'), { target: { value: 'jetpack' } });
    expect(screen.getByRole('heading', { name: 'No mods for “jetpack”' })).toBeVisible();
    await click('Clear search');
    expect(mods.bQuery).toBe('');
    mods.bTag = 'nothing';
    await waitFor(() => expect(screen.getByRole('heading', { name: 'No mods match these filters' })).toBeVisible());
    mods.bTag = null;
    await waitFor(() => screen.getByRole('button', { name: 'Pet Companions' }));
    await click('Pet Companions');
    expect(screen.getByRole('heading', { name: /Pet Companions/ })).toBeVisible();
    await click('github.com/lumen/sky-core');
    await click('Report this mod to the maintainers');
    expect(mock.api.openUrl).toHaveBeenCalledWith('https://github.com/Blockheads-Club/mods/issues/new');
    await click('Install');
    await click('Mod list');
    expect(mods.bDetail).toBeNull();
  });

  it('shows details of installed, native and incompatible mods', async () => {
    mods.browse = makeBrowse({ mods: [
      makeBrowseMod({ id: 'a', installed: '1.2.0', depends: { base: '^1' }, size: null, tags: [], published: '', versions: [['1.2.0', '']] }),
      makeBrowseMod({ id: 'b', name: 'B', compatible: false, compatible_version: null, native: true, permissions: ['native'] }),
      makeBrowseMod({ id: 'c', name: 'C', compatible: false, compatible_version: '1.0.0' })
    ] });
    mods.bDetail = 'a';
    const r = render(ModsBrowse);
    expect(screen.getByText('Installed')).toBeVisible();
    expect(screen.getByText('base')).toBeVisible();
    r.unmount();
    mods.bDetail = 'b';
    const r2 = render(ModsBrowse);
    expect(screen.getByText('No version works with your Blockheads PC yet. Update Blockheads PC first.')).toBeVisible();
    await click('Install');
    r2.unmount();
    mods.bDetail = 'c';
    render(ModsBrowse);
    expect(screen.getByText(/Version 1.0.0 works with yours/)).toBeVisible();
  });

  it('shows loading, offline, errors and an empty list', async () => {
    mods.browseLoading = true;
    const r = render(ModsBrowse);
    expect(screen.getByRole('status')).toHaveTextContent('Loading the mod list…');
    r.unmount();
    mods.browseLoading = false;
    mods.browseError = { code: 'offline', detail: 'dns' };
    const r2 = render(ModsBrowse);
    expect(screen.getByRole('alert')).toHaveTextContent('Can’t reach the mod list');
    await click('Install from file');
    expect(mods.install?.stage).toBe('source');
    await click('Try again');
    await waitFor(() => expect(mock.api.modsIndex).toHaveBeenCalledWith(true));
    r2.unmount();
    mods.browseError = { code: 'other', detail: 'boom' };
    mods.browse = makeBrowse({ offline: true, mods: [], tags: [] });
    const r3 = render(ModsBrowse);
    expect(screen.getByRole('alert')).toHaveTextContent('Something went wrong: boom');
    expect(screen.getByText(/showing the copy from 2026-10-21/)).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'The mod list is empty for now' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Clear search' })).toBeNull();
    await click('Try again');
    r3.unmount();
    mods.browseError = { code: 'index' };
    render(ModsBrowse);
    expect(screen.getByRole('alert')).toHaveTextContent('Can’t reach');
  });
});

describe('Server mods', () => {
  it('lists servers and clears them', async () => {
    mods.servers = makeServers();
    render(ModsServers);
    expect(screen.getByText('Also used by Dodo Land')).toBeVisible();
    await click('Clear the mods of Sunny Isle');
    expect(mock.api.modsClearServer).toHaveBeenCalledWith('mira.example.net_15151');
    mods.servers = makeServers();
    await waitFor(() => screen.getByRole('button', { name: /Clear all/ }));
    await click(/Clear all \(4\.9 MB\)/);
    expect(mock.api.modsClearServers).toHaveBeenCalled();
    mods.servers = makeServers();
    await waitFor(() => screen.getAllByRole('group'));
    const second = document.querySelectorAll('details')[1];
    second.open = true;
    await fireEvent(second, new Event('toggle'));
    await click('Storage limit in Settings → Mods');
    expect(store.section).toBe('mods');
  });

  it('shows loading, empty and error states', async () => {
    const r = render(ModsServers);
    expect(screen.getByRole('status')).toBeTruthy();
    r.unmount();
    mods.servers = makeServers({ servers: [], total: 0 });
    const r2 = render(ModsServers);
    expect(screen.getByRole('heading', { name: 'No server mods' })).toBeVisible();
    r2.unmount();
    mods.serversError = { code: 'io' };
    render(ModsServers);
    await click('Try again');
    expect(mock.api.modsServers).toHaveBeenCalled();
  });
});

describe('Developer', () => {
  it('offers to turn developer mode on', async () => {
    render(ModsDev);
    await click('Read the mod guide');
    expect(mock.api.openUrl).toHaveBeenCalled();
    await click('Turn on developer mode');
    expect(store.prefs.mods_developer).toBe(true);
  });

  it('loads folders, filters the log and reloads', async () => {
    store.prefs = { ...store.prefs, mods_developer: true, mods_dev_folders: ['/code/my-mod', '/code/b'] };
    mock.api.modsDevInspect = vi.fn(async (path: string) =>
      path === '/code/b' ? makeDevMod({ path, id: null, name: null, version: null, native: true, errors: [], warnings: ['w1'] }) : makeDevMod({ path, errors: ['e1', 'e2'] })
    );
    render(ModsDev);
    await screen.findByText('2 problems');
    expect(screen.getByRole('log')).toHaveTextContent('my-mod: e1');
    expect(screen.getByRole('log')).toHaveTextContent('/code/b: w1');
    await click(/Errors 2/);
    expect(screen.getByRole('log')).not.toHaveTextContent('w1');
    await click(/Warnings 1/);
    expect(screen.getByRole('log')).not.toHaveTextContent('e1');
    await click('All');
    await click('Copy error');
    expect(clipboard).toHaveBeenCalledWith('e1\ne2');
    await click('Open folder');
    expect(mock.api.openPath).toHaveBeenCalledWith('/code/my-mod');
    await click(/Reload/);
    await fireEvent.keyDown(window, { key: 'r', ctrlKey: true });
    await fireEvent.keyDown(window, { key: 'x', ctrlKey: true });
    await waitFor(() => expect(mock.api.modsDevReload).toHaveBeenCalledTimes(2));
    await fireEvent.click(screen.getByRole('switch', { name: 'Reload when files change' }));
    expect(store.prefs.mods_dev_watch).toBe(false);
    await click(/Unnamed mod/);
    await click('Unload');
    await waitFor(() => expect(store.prefs.mods_dev_folders).toEqual(['/code/my-mod']));
    await click('Load from folder…');
    expect(mock.api.pickFolder).toHaveBeenCalled();
  });

  it('shows the running folder mods, the game log and joins localhost', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    store.prefs = { ...store.prefs, mods_developer: true, mods_dev_folders: ['/code/my-mod'] };
    store.game = { ...store.game, state: 'ready', mods: [{ id: 'my-mod', version: '0.1.0-dev', state: 'running', origin: 'dev', ms: 3.6, peak_ms: 4, calls: 9 }, { id: 'mine', version: '1', state: 'running', origin: 'mine', ms: 0, peak_ms: 0, calls: 1 }] };
    store.log = ['[mods]     5 ms error my-mod: boom', '[mods]     6 ms warn  my-mod: slow', '[mods]     7 ms info  ok'];
    render(ModsDev);
    await screen.findByText(/my-mod: 3.6 ms of mod code per second/);
    expect(screen.getByRole('log')).toHaveTextContent('my-mod: boom');
    await click(/Errors 1/);
    expect(screen.getByRole('log')).not.toHaveTextContent('ok');
    await click(/Warnings 1/);
    expect(screen.getByRole('log')).toHaveTextContent('slow');
    await click('Start game and join localhost');
    expect(mock.api.modsDevJoin).toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1100);
    vi.useRealTimers();
  });

  it('shows the empty folder list and ignores Ctrl+R without a selection', async () => {
    store.prefs = { ...store.prefs, mods_developer: true };
    render(ModsDev);
    expect(screen.getByText(/No folder mods yet/)).toBeVisible();
    expect(screen.getByRole('log')).toHaveTextContent('Nothing yet');
    await fireEvent.keyDown(window, { key: 'R', metaKey: true });
    store.prefs = { ...store.prefs, mods_developer: false };
    await fireEvent.keyDown(window, { key: 'r', ctrlKey: true });
    expect(mock.api.modsDevReload).not.toHaveBeenCalled();
  });
});

describe('Install dialog', () => {
  it('walks a file or link through the checks', async () => {
    mods.startFile();
    render(InstallFlow);
    const next = screen.getByRole('button', { name: 'Next' });
    expect(next).toBeDisabled();
    await click('Choose file');
    await waitFor(() => expect(mock.api.modsInspectFile).toHaveBeenCalled());
    mods.startFile();
    await waitFor(() => screen.getByLabelText('Or paste a link'));
    const link = screen.getByLabelText('Or paste a link');
    await fireEvent.input(link, { target: { value: 'https://e.test/a.bhmod' } });
    await fireEvent.keyDown(link, { key: 'a' });
    await fireEvent.keyDown(link, { key: 'Enter' });
    await waitFor(() => expect(mock.api.modsInspectLink).toHaveBeenCalledWith('https://e.test/a.bhmod'));
    mods.startFile();
    await waitFor(() => screen.getByLabelText('Or paste a link'));
    await fireEvent.keyDown(screen.getByLabelText('Or paste a link'), { key: 'Enter' });
    await fireEvent.input(screen.getByLabelText('Or paste a link'), { target: { value: 'https://e.test/b.bhmod' } });
    await click('Next');
    await waitFor(() => expect(mock.api.modsInspectLink).toHaveBeenCalledTimes(2));
    // Dropping a file while it asks for one; other stages ignore drops.
    mods.startFile();
    hooks.drop!(['/d.bhmod']);
    await waitFor(() => expect(mods.install?.stage).toBe('perms'));
    expect(mock.api.modsInspectFile).toHaveBeenCalledWith('/d.bhmod');
    hooks.drop!(['/e.bhmod']);
    mods.startFile();
    hooks.drop!([]);
    expect(mock.api.modsInspectFile).toHaveBeenCalledTimes(2);
    await click('Cancel');
    expect(mods.install).toBeNull();
  });

  it('survives a window without file drops', async () => {
    mock.win.onFileDrop = vi.fn(async () => Promise.reject(new Error('no')));
    mods.startFile();
    const r = render(InstallFlow);
    r.unmount();
  });

  it('shows the plan, permissions and install of index mods', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({
      items: [makePlanItem({ permissions: ['world:read', 'players:kick'], replaces: '0.9.0' }), makePlanItem({ id: 'sky-core', name: 'Sky Core', size: null, needed_by: 'day-sky', permissions: [], reviewed: false })],
      missing: [{ id: 'gone', req: '^1', needed_by: 'day-sky' }], satisfied: [{ id: 'base', req: '^1', version: '1.0.0' }]
    }));
    await mods.installFromIndex('day-sky');
    render(InstallFlow);
    expect(screen.getByText('Step 1 of 2 · What gets installed')).toBeVisible();
    expect(screen.getByText(/gone \^1 \(needed by day-sky\)/)).toBeVisible();
    expect(screen.getByText(/1 needed mod is already installed/)).toBeVisible();
    await click('Cancel');
    await mods.installFromIndex('day-sky');
    await waitFor(() => screen.getByRole('button', { name: 'Next' }));
    await click('Next');
    expect(screen.getByText('Step 2 of 2 · Review what they may do')).toBeVisible();
    expect(screen.getByRole('note')).toHaveTextContent('Not reviewed');
    const kick = screen.getByRole('checkbox', { name: /Kick players/ });
    await fireEvent.click(kick);
    expect(mods.install?.grants['day-sky']).toEqual(['world:read']);
    await click('Back');
    expect(mods.install?.stage).toBe('plan');
    await click('Next');
    let finish!: (v: string[]) => void;
    mock.api.modsInstall = vi.fn(() => new Promise<string[]>((r) => (finish = r)));
    let report!: (p: unknown) => void;
    mock.api.onModsInstall = vi.fn(async (cb: (p: unknown) => void) => ((report = cb), () => {}));
    await click('Install, keep off');
    await waitFor(() => expect(mock.api.modsInstall).toHaveBeenCalled());
    expect(screen.getByRole('progressbar')).toBeTruthy();
    report({ stage: 'downloading', done: 1, total: 2 });
    await waitFor(() => expect(screen.getByText('Downloading (1 of 2)')).toBeVisible());
    report({ stage: 'checking', done: 2, total: 2 });
    report({ stage: 'writing', done: 2, total: 0 });
    await waitFor(() => expect(screen.getByText('Saving')).toBeVisible());
    finish(['day-sky']);
    await screen.findByRole('heading', { name: 'Day & Night Sky is ready' });
    await click('Open in My mods');
    expect(mods.tab).toBe('mine');
  });

  it('asks twice for native code, starting on the safe button', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem({ id: 'turbo', name: 'Turbo Lighting', native: true, permissions: ['native'], authors: [] })] }));
    await mods.installFromIndex('turbo');
    render(InstallFlow);
    expect(screen.getByText('Runs outside the sandbox. You decide on the next step.')).toBeVisible();
    await click('Cancel');
    await mods.installFromIndex('turbo');
    await waitFor(() => screen.getByRole('button', { name: 'Install and turn on' }));
    await click('Install and turn on');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Don’t install' })).toHaveFocus());
    expect(screen.getByText(/The maintainers reviewed it/)).toBeVisible();
    const allow = screen.getByRole('button', { name: 'Allow full access and install' });
    expect(allow).toBeDisabled();
    await fireEvent.click(screen.getByLabelText('I understand what native code can do.'));
    await fireEvent.click(screen.getByLabelText('I trust the author with my game and this computer.'));
    expect(allow).toBeEnabled();
    await fireEvent.click(allow);
    await waitFor(() => expect(mock.api.modsInstall).toHaveBeenCalledWith(expect.objectContaining({ native_confirmed: true })));
    await fireEvent.click(screen.getAllByRole('button', { name: 'Close' }).at(-1)!);
    // Unreviewed, from a named author; then blocked by the setting.
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem({ id: 't', name: 'T', native: true, permissions: ['native'], reviewed: false })] }));
    await mods.installFromIndex('t');
    await mods.confirm(true);
    expect(screen.getAllByText(/nobody has reviewed it/)[0]).toBeVisible();
    expect(screen.getByLabelText(/I trust lumen/)).toBeTruthy();
    await click('Don’t install');
    expect(mods.install).toBeNull();
    store.prefs = { ...store.prefs, mods_block_native: true };
    await mods.installFromIndex('t');
    await mods.confirm(true);
    expect(screen.getByRole('alert')).toHaveTextContent('Native mods are blocked');
    await click('Open Settings');
    expect(store.section).toBe('mods');
  });

  it('shows check failures, retries and the newer-loader choice', async () => {
    mods.startFile();
    mods.install!.link = 'https://e.test/x.bhmod';
    mock.api.modsInspectLink = vi.fn(async () => Promise.reject({ code: 'checksum', file: 'x.bhmod', expected: 'aa', got: 'bb' }));
    await mods.inspectLink();
    const r = render(InstallFlow);
    expect(screen.getByRole('heading', { name: 'The file check failed' })).toBeVisible();
    expect(screen.getByText(/expected aa/)).toBeTruthy();
    await click('Copy details');
    expect(clipboard).toHaveBeenCalled();
    await click('Try again');
    expect(mock.api.modsInspectLink).toHaveBeenCalledTimes(2);
    r.unmount();
    mods.install = { ...mods.install!, stage: 'failed', error: { code: 'size', got: '2' }, retry: null };
    const r2 = render(InstallFlow);
    expect(screen.getByText(/got 2/)).toBeTruthy();
    r2.unmount();
    mods.install = { ...mods.install!, stage: 'failed', error: { code: 'invalid', detail: 'x' }, retry: null };
    const r3 = render(InstallFlow);
    expect(screen.getByRole('heading', { name: 'Not installed' })).toBeVisible();
    await fireEvent.click(screen.getAllByRole('button', { name: 'Close' }).at(-1)!);
    r3.unmount();
    mock.api.modsPlan = vi.fn(async () => Promise.reject({ code: 'newerLoader', file: 'Key Hints 2.0.0', expected: '^1.4', detail: '1.6.0' }));
    await mods.installFromIndex('keys');
    const r4 = render(InstallFlow);
    expect(screen.getByText(/Key Hints 2.0.0 needs mod loader \^1.4/)).toBeVisible();
    await click('Update Blockheads PC');
    expect(mock.api.openUrl).toHaveBeenCalledWith('https://github.com/Blockheads-Club/blockheads-pc/releases');
    mock.api.modsPlan = makeApi().modsPlan;
    await click('Install 1.6.0 instead');
    await waitFor(() => expect(mods.install?.stage).toBe('plan'));
    r4.unmount();
    mods.install = { ...mods.install!, stage: 'newer', newer: { id: 'k', name: 'K', needs: '^2', alt: null } };
    const r5 = render(InstallFlow);
    expect(screen.queryByRole('button', { name: /instead/ })).toBeNull();
    await click('Cancel');
    r5.unmount();
    mods.startFile();
    mods.install!.stage = 'checking';
    render(InstallFlow);
    expect(screen.getByText('Checking the file and what it needs…')).toBeVisible();
  });
});

describe('small parts', () => {
  it('draws every status and badge', () => {
    for (const s of ['running', 'ready', 'off', 'failed', 'blocked', 'budget', 'dependency', 'permissions'] as const) render(ModStatus, { status: s });
    expect(screen.getByText('Running')).toBeVisible();
    render(Badges, { kind: 'update', version: '2.0' });
    expect(screen.getByText('Update 2.0')).toBeVisible();
  });
});

describe('more cases', () => {
  it('switches tabs with the controller bumpers', async () => {
    const frames: (() => void)[] = [];
    vi.stubGlobal('requestAnimationFrame', (cb: () => void) => frames.push(cb));
    vi.stubGlobal('cancelAnimationFrame', vi.fn());
    const pad = { index: 0, axes: [0, 0], buttons: Array.from({ length: 16 }, (_, i) => ({ pressed: i === 5 })) };
    Object.defineProperty(navigator, 'getGamepads', { value: () => [pad], configurable: true });
    const r = render(ModsScreen);
    frames.shift()!();
    expect(mods.tab).toBe('browse');
    r.unmount();
    Object.defineProperty(navigator, 'getGamepads', { value: undefined, configurable: true });
    vi.unstubAllGlobals();
  });

  it('covers the settings page', async () => {
    render(SettingsMods);
    await waitFor(() => expect(screen.getByText('4.9 MB from 2 servers', { exact: false })).toBeTruthy());
    await fireEvent.change(screen.getByRole('combobox'), { target: { value: '0' } });
    await waitFor(() => expect(store.prefs.mods_cache_limit_mb).toBe(0));
    expect(mock.api.modsServers).toHaveBeenCalledTimes(2);
    await fireEvent.click(screen.getByRole('switch', { name: 'Update mods by themselves' }));
    expect(store.prefs.mods_auto_update).toBe(false);
    await fireEvent.click(screen.getByRole('switch', { name: 'Never allow native mods' }));
    await waitFor(() => expect(store.prefs.mods_block_native).toBe(true));
    // JVM mods: off by default, the Java folder field is locked until the switch is on.
    const java = screen.getByLabelText('Java folder (optional)') as HTMLInputElement;
    expect(store.prefs.mods_jvm).toBe(false);
    expect(java.disabled).toBe(true);
    expect(screen.getByRole('note')).toHaveTextContent('cannot be sandboxed');
    await fireEvent.click(screen.getByRole('switch', { name: 'Run Java and Kotlin mods' }));
    await waitFor(() => expect(store.prefs.mods_jvm).toBe(true));
    expect(java.disabled).toBe(false);
    await fireEvent.change(java, { target: { value: ' /opt/jre ' } });
    await waitFor(() => expect(store.prefs.mods_java_home).toBe('/opt/jre'));
    await fireEvent.click(screen.getByRole('switch', { name: 'Developer mode' }));
    expect(store.prefs.mods_developer).toBe(true);
    await click('Open');
    expect(mock.api.openPath).toHaveBeenCalledWith('/data/mods');
    await click('Manage…');
    expect(mods.tab).toBe('servers');
    await click('Open Mods');
    expect(mods.tab).toBe('mine');
  });

  it('covers the rest of the details and list branches', async () => {
    const r = render(ModDetail, { m: makeMod({ authors: [], source: '', installed_at: 0, permissions: ['world:write'], granted: [], dependencies: [{ id: 's', req: '^1', installed: null, ok: false, soft: true, available: null }] }) });
    expect(screen.getByText('· optional')).toBeTruthy();
    await r.rerender({ m: makeMod({ update: '2.0.0' }) });
    await click('Update 2.0.0');
    r.unmount();
    mods.browse = makeBrowse({ mods: [
      makeBrowseMod({ id: 'x', name: 'X', size: null, permissions: ['players:kick', 'world:write'] }),
      makeBrowseMod({ id: 'y', name: 'Y', installed: '1.0.0', version: '1.2.0', description: null })
    ] });
    mods.bDetail = 'y';
    const r2 = render(ModsBrowse);
    await click('Update');
    r2.unmount();
    mods.bDetail = null;
    render(ModsBrowse);
    expect(screen.getByText('Kick players')).toBeVisible();
    render(PermissionChip, { perm: 'players:ban' });
  });

  it('covers the rest of the install dialog branches', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem({ native: true, permissions: ['world:write', 'native'] })], missing: [{ id: 'g', req: '^1', needed_by: 'day-sky' }] }));
    await mods.installFromIndex('day-sky');
    render(InstallFlow);
    expect(screen.getByText('Just this mod.')).toBeVisible();
    await click('Next');
    expect(screen.getByRole('checkbox', { name: /Change the world/ })).toBeChecked();
    mods.install = { ...mods.install!, stage: 'installing', progress: { stage: 'checking', done: 1, total: 1 } };
    await waitFor(() => expect(screen.getByText('Downloading (1 of 1)')).toBeVisible());
    mods.install = { ...mods.install!, stage: 'failed', error: { code: 'checksum', expected: 'e' }, retry: null };
    await waitFor(() => expect(screen.getByText(/expected e/)).toBeTruthy());
  });

  it('shows dev folder warnings of named mods', async () => {
    store.prefs = { ...store.prefs, mods_developer: true, mods_dev_folders: ['/c'] };
    mock.api.modsDevInspect = vi.fn(async (path: string) => makeDevMod({ path, warnings: ['w'] }));
    render(ModsDev);
    await waitFor(() => expect(screen.getByRole('log')).toHaveTextContent('my-mod: w'));
  });
});

describe('last branches', () => {
  it('closes dialogs with Esc and uses the narrow install button', async () => {
    viewport.modsNarrow = true;
    render(ModsScreen);
    await click('Install from file or link');
    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(mods.install).toBeNull();
    viewport.modsNarrow = false;
  });

  it('cancels a single-mod permission review and closes the remove question with Esc', async () => {
    mock.api.modsPlan = vi.fn(async () => makePlan({ items: [makePlanItem()] }));
    await mods.installFromIndex('day-sky');
    const r = render(InstallFlow);
    await click('Cancel');
    expect(mods.install).toBeNull();
    r.unmount();
    render(ModDetail, { m: makeMod({ update: '1.0.0', permissions: ['players:ban', 'native'], granted: [] }) });
    await fireEvent.click(screen.getAllByRole('button', { name: 'Update' })[0]);
    expect(mock.api.modsPlan).toHaveBeenCalledWith('minimap', '1.0.0');
    await click('Remove');
    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('cancels the file step', async () => {
    mods.startFile();
    render(InstallFlow);
    await click('Cancel');
    expect(mods.install).toBeNull();
  });

  it('shows the sides hints on cards and unnamed folder errors', async () => {
    mods.bAll = true;
    mods.browse = makeBrowse({ mods: [makeBrowseMod({ id: 's', name: 'S', sides: ['server'] }), makeBrowseMod({ id: 'b', name: 'B', sides: ['server', 'client'] }), makeBrowseMod({ id: 'c', name: 'C' })] });
    render(ModsBrowse);
    expect(screen.getByText(/installs on a Blockheads Server/)).toBeTruthy();
    expect(screen.getByText(/Works fully only/)).toBeTruthy();
    store.prefs = { ...store.prefs, mods_developer: true, mods_dev_folders: ['/z'] };
    mock.api.modsDevInspect = vi.fn(async (path: string) => makeDevMod({ path, id: null, errors: ['bad'] }));
    render(ModsDev);
    await waitFor(() => expect(screen.getByRole('log')).toHaveTextContent('/z: bad'));
  });
});
