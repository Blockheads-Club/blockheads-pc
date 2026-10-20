import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('../api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import { makeApi, makeConfig, makeSnap, makeWin } from '../mock-api';
import { store } from '../store.svelte';
import { resetStore } from '../test-utils';
import { setTheme } from '../theme.svelte';
import { viewport } from '../viewport.svelte';
import CloseDialog from './CloseDialog.svelte';
import Home from './Home.svelte';
import ImportDialog from './ImportDialog.svelte';
import JoinDialog from './JoinDialog.svelte';
import LogsDialog from './LogsDialog.svelte';
import Playing from './Playing.svelte';
import UpdateDialog from './UpdateDialog.svelte';

const click = (name: string | RegExp) => fireEvent.click(screen.getByRole('button', { name }));
const clipboard = vi.fn(async () => {});

beforeEach(() => {
  mock.api = makeApi();
  mock.win = makeWin().win;
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: clipboard }, configurable: true });
  clipboard.mockClear();
  resetStore();
  viewport.compact = false;
});
afterEach(() => cleanup());

describe('Home', () => {
  it('is ready to play: Play starts the game', async () => {
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'starting', running_secs: 0 }));
    render(Home);
    expect(screen.getByRole('heading', { name: /The Blockheads/ })).toHaveTextContent('1.7.6');
    expect(screen.getByRole('status', { name: 'Game status: Ready to play' })).toBeVisible();
    await click('Play');
    expect(mock.api.gameStart).toHaveBeenCalled();
    await waitFor(() => expect(store.screen).toBe('playing'));
  });

  it('shows the launch progress', () => {
    store.game = makeSnap({ state: 'starting', running_secs: 0 });
    const { unmount: u0 } = render(Home);
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
    u0();
    store.game = makeSnap({ state: 'starting', running_secs: 0, progress: { percent: 42.7, phase: 'constructors', step: 3, elapsed_secs: 4, eta_secs: null, slow: false, stalled: false, step_secs: [], ready: false } });
    render(Home);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '42');
    cleanup();
    render(Home);
    expect(screen.getByRole('button', { name: 'Starting…' })).toBeDisabled();
    expect(screen.getByRole('progressbar', { name: 'Starting the game' })).toBeVisible();
  });

  it('lets a running game be rejoined or stopped', async () => {
    store.game = makeSnap({ state: 'running', running_secs: 720 });
    render(Home);
    expect(screen.getByRole('status', { name: 'Game status: Running · 12 min' })).toBeVisible();
    expect(screen.getByText(/still running/)).toBeVisible();
    await click('Back to game');
    expect(store.screen).toBe('playing');
    store.screen = 'home';
    await click('Stop game');
    expect(mock.api.gameStop).toHaveBeenCalled();
  });

  it('offers Play again after a normal stop, with the last session', async () => {
    store.game = makeSnap({ state: 'stopped', last_session_secs: 3840 });
    render(Home);
    expect(screen.getByText('Last session: 1 h 04 min · closed normally')).toBeVisible();
    await click('Play again');
    expect(mock.api.gameStart).toHaveBeenCalled();
    cleanup();
    store.game = makeSnap({ state: 'stopped', last_session_secs: null });
    render(Home);
    expect(screen.queryByText(/Last session/)).toBeNull();
  });

  it('explains a crash and offers the log', async () => {
    store.log = ['a', 'b', 'c', 'd'];
    store.game = makeSnap({ state: 'crashed', exit_code: 139 });
    render(Home);
    expect(screen.getByRole('alert')).toHaveTextContent('The game closed unexpectedly');
    const log = screen.getByLabelText('Last log lines');
    expect(log.textContent).toBe('b\nc\nd\nexit     code 139');
    await click('Copy log');
    expect(clipboard).toHaveBeenCalled();
    await click('Save log…');
    expect(mock.api.saveLog).toHaveBeenCalled();
    await click('Report on GitHub');
    expect(mock.api.openUrl).toHaveBeenCalledWith(expect.stringContaining('/issues/new'));
    await click('Play again');
    expect(mock.api.gameStart).toHaveBeenCalled();
    cleanup();
    store.game = makeSnap({ state: 'crashed', signal: 11 });
    store.log = [];
    render(Home);
    expect(screen.getByLabelText('Last log lines').textContent).toBe('exit     signal 11');
    cleanup();
    store.game = makeSnap({ state: 'crashed' });
    render(Home);
    expect(screen.getByLabelText('Last log lines').textContent).toBe('');
  });

  it('asks for the game file when it is gone', async () => {
    store.game = makeSnap({ state: 'missing' });
    render(Home);
    expect(screen.getByRole('alert')).toHaveTextContent('We can’t find your game file');
    expect(screen.getByRole('button', { name: 'Play' })).toBeDisabled();
    await click('Choose game file…');
    expect(mock.api.pickFile).toHaveBeenCalled();
  });

  it('runs the quick actions, the top bar and the update pill', async () => {
    store.update = { stage: 'available', info: { version: '0.2.0', size_mb: 43, notes: [] }, progress: null, error: '', checked: true };
    render(Home);
    await click(/Update available/);
    expect(store.dialog).toBe('update');
    await click('Dark mode');
    expect(mock.api.savePrefs).toHaveBeenLastCalledWith(expect.objectContaining({ theme: 'dark' }));
    await click('Light mode');
    await click('Join a server');
    expect(store.dialog).toBe('join');
    await click('Settings');
    expect(store.screen).toBe('settings');
    store.screen = 'home';
    await click(/Change game file/);
    expect(store.screen).toBe('settings');
    store.screen = 'home';
    await click(/Import saves/);
    expect(store.dialog).toBe('import');
    await click(/Open data folder/);
    expect(mock.api.openPath).toHaveBeenCalledWith('/data');
    await click(/View logs/);
    expect(store.dialog).toBe('logs');
    await click(/How to join a server/);
    expect(store.dialog).toBe('join');
    await click('Legal notice');
    expect(store.section).toBe('about');
  });

  it('has a compact layout for small windows', async () => {
    viewport.compact = true;
    store.update = { stage: 'downloading', info: { version: '0.2.0', size_mb: 43, notes: [] }, progress: null, error: '', checked: true };
    setTheme('dark');
    render(Home);
    expect(screen.getByRole('navigation', { name: 'Quick actions' })).toBeVisible();
    expect(screen.queryByText('0.2.0')).toBeNull();
    await click('Switch to light mode');
    setTheme('light');
    cleanup();
    render(Home);
    await click('Switch to dark mode');
    await click('How to join a server');
    expect(store.dialog).toBe('join');
    await click('Game file');
    cleanup();
    for (const state of ['running', 'crashed', 'stopped', 'starting', 'missing'] as const) {
      store.game = makeSnap({ state, running_secs: 5, exit_code: 1 });
      const { unmount } = render(Home);
      expect(screen.getByRole('status', { name: /Game status/ })).toBeVisible();
      unmount();
    }
  });

  it('has no fullscreen on Home: only the game goes fullscreen', () => {
    render(Home);
    expect(screen.queryByRole('button', { name: 'Fullscreen' })).toBeNull();
  });

  it('copes without a saved configuration or a known version', () => {
    store.config = null;
    store.apk = null;
    store.game = makeSnap({ state: 'missing' });
    render(Home);
    expect(screen.getByRole('heading', { name: 'The Blockheads' })).not.toHaveTextContent('1.7');
    expect(screen.getByText(/“” was moved/)).toBeVisible();
  });

  it('hides the update pill without an update', () => {
    render(Home);
    expect(screen.queryByText('Update available')).toBeNull();
  });
});

describe('Playing', () => {
  it('shows the running game and the controls', async () => {
    store.screen = 'playing';
    store.game = makeSnap({ state: 'running', running_secs: 120 });
    render(Playing);
    expect(screen.getByText('The game runs in its own window')).toBeVisible();
    expect(screen.getByRole('list', { name: 'Controls in the game' })).toBeVisible();
    await click('Home');
    expect(store.screen).toBe('home');
    expect(screen.queryByRole('button', { name: 'Fullscreen' })).toBeNull(); // the game has its own window here
    await click('How to join a server');
    expect(store.dialog).toBe('join');
    await click('View logs');
    expect(store.dialog).toBe('logs');
    await click('Stop game');
    expect(mock.api.gameStop).toHaveBeenCalled();
    cleanup();
    store.config = null;
    render(Playing);
    expect(screen.queryByRole('list')).toBeNull();
  });
});

describe('Playing inside the window', () => {
  const top = () => mock.api.embedLayout.mock.calls.map((c: number[]) => c[0]);

  it('leaves the stage to the native game view and reports where it starts', async () => {
    store.screen = 'playing';
    store.game = makeSnap({ state: 'running', running_secs: 5, embedded: true });
    const { unmount } = render(Playing);
    expect(screen.queryByText('The game runs in its own window')).toBeNull();
    await waitFor(() => expect(top()).toEqual([0]));
    // A dialog needs the whole window; closing it gives the stage back.
    store.dialog = 'close';
    await waitFor(() => expect(top().at(-1)).toBe(-1));
    store.dialog = null;
    await waitFor(() => expect(top().at(-1)).toBe(0));
    // The strip's Fullscreen button makes the game fullscreen, which hides the strip.
    await fireEvent.click(screen.getByRole('button', { name: 'Fullscreen' }));
    expect(mock.api.embedFullscreen).toHaveBeenCalledWith(true);
    expect(screen.getByRole('button', { name: 'Stop game' })).toBeVisible();
    store.fullscreen = true;
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Stop game' })).toBeNull());
    store.fullscreen = false;
    unmount();
    expect(top().at(-1)).toBe(-1);
  });

  it('survives a backend that cannot lay out the view', async () => {
    mock.api.embedLayout = vi.fn(async () => {
      throw new Error('no view');
    });
    mock.api.embedReveal = vi.fn(async () => {
      throw new Error('no view');
    });
    store.screen = 'playing';
    // Already running when shown (no loader): the game is revealed at once.
    store.game = makeSnap({ state: 'running', running_secs: 5, embedded: true, progress: { ready: true } as never });
    const { unmount } = render(Playing);
    await waitFor(() => expect(mock.api.embedLayout).toHaveBeenCalled());
    await waitFor(() => expect(mock.api.embedReveal).toHaveBeenCalled());
    unmount();
    expect(mock.api.embedLayout).toHaveBeenCalledTimes(2);
  });
});

describe('dialogs', () => {
  it('close with Escape', async () => {
    for (const [d, C] of [['join', JoinDialog], ['logs', LogsDialog], ['close', CloseDialog], ['import', ImportDialog], ['update', UpdateDialog]] as const) {
      store.dialog = d;
      store.update = { stage: 'available', info: { version: '1', size_mb: 1, notes: [] }, progress: null, error: '', checked: true };
      const { unmount } = render(C as never);
      await fireEvent.keyDown(screen.getByRole(d === 'close' ? 'alertdialog' : 'dialog').parentElement!, { key: 'Escape' });
      expect(store.dialog).toBeNull();
      unmount();
    }
  });

  it('explains how to join a server', async () => {
    store.dialog = 'join';
    render(JoinDialog);
    expect(screen.getByRole('dialog', { name: 'How to join a server' })).toBeVisible();
    await click(/Guide: play with friends/);
    expect(mock.api.openUrl).toHaveBeenCalled();
    await click('Got it');
    expect(store.dialog).toBeNull();
  });

  it('shows the log and copies or saves it', async () => {
    store.dialog = 'logs';
    const { unmount } = render(LogsDialog);
    expect(screen.getByRole('log')).toHaveTextContent('Nothing yet');
    expect(screen.getByRole('button', { name: 'Copy log' })).toBeDisabled();
    unmount();
    store.log = ['one', 'two'];
    render(LogsDialog);
    expect(screen.getByRole('log').textContent).toBe('one\ntwo');
    await click('Copy log');
    expect(clipboard).toHaveBeenCalledWith('one\ntwo');
    await click('Save log…');
    expect(mock.api.saveLog).toHaveBeenCalled();
    store.log = ['one', 'two', 'three'];
    await waitFor(() => expect(screen.getByRole('log')).toHaveTextContent('three'));
    await fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[1]);
    expect(store.dialog).toBeNull();
  });

  it('confirms closing while playing', async () => {
    store.dialog = 'close';
    render(CloseDialog);
    expect(screen.getByRole('alertdialog')).toHaveTextContent('Stop the game and quit Blockheads PC?');
    await click('Keep playing');
    expect(store.dialog).toBeNull();
    store.dialog = 'close';
    await click('Stop and quit');
    await waitFor(() => expect(mock.api.appQuit).toHaveBeenCalled());
  });

  it('imports saves from Home', async () => {
    store.dialog = 'import';
    render(ImportDialog);
    await click('Browse…');
    await waitFor(() => expect(store.importPath).toBe('/dl/saves'));
    await click('Import');
    await waitFor(() => expect(store.importSummary?.written).toBe(3));
    await fireEvent.click(screen.getAllByRole('button', { name: 'Close' })[1]);
    expect(store.dialog).toBeNull();
  });
});

describe('UpdateDialog', () => {
  const info = { version: '0.2.0', size_mb: 43, notes: ['Faster', 'Fixes'] };
  const set = (stage: any, extra: Record<string, unknown> = {}) =>
    (store.update = { stage, info, progress: null, error: '', checked: true, ...extra });

  it('offers the update and installs it', async () => {
    set('available');
    render(UpdateDialog);
    expect(screen.getByRole('dialog', { name: 'Blockheads PC 0.2.0 is here' })).toBeVisible();
    expect(screen.getByText('Faster')).toBeVisible();
    expect(screen.getByText(/Download size 43 MB/)).toBeVisible();
    await click('Later');
    expect(store.dialog).toBeNull();
    await click('Download and install');
    expect(mock.api.updateInstall).toHaveBeenCalled();
  });

  it('shows download progress with and without numbers', async () => {
    set('downloading');
    render(UpdateDialog);
    expect(screen.getByRole('progressbar', { name: 'Download progress' })).toHaveAttribute('aria-valuenow', '0');
    cleanup();
    set('downloading', { progress: { stage: 'downloading', percent: 42, done_mb: 18, total_mb: 43, mb_per_s: 1.4, eta_s: 30 } });
    render(UpdateDialog);
    expect(screen.getByText('18 / 43 MB · 1.4 MB/s · about 30 s left')).toBeVisible();
    await click('Hide');
    await click('Cancel download');
    expect(store.update.stage).toBe('available');
  });

  it('shows the install steps', () => {
    set('installing');
    render(UpdateDialog);
    expect(screen.getByText('Downloaded and verified')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Restart now' })).toBeDisabled();
  });

  it('finishes with a restart, or waits while the game runs', async () => {
    set('ready');
    render(UpdateDialog);
    expect(screen.getByRole('dialog', { name: 'Update installed' })).toBeVisible();
    await click('Restart later');
    await click('Restart now');
    expect(mock.win.close).toHaveBeenCalled();
    cleanup();
    store.game = makeSnap({ state: 'running' });
    set('ready');
    render(UpdateDialog);
    expect(screen.getByText(/The game is running/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Restart now' })).toBeDisabled();
    await click('OK');
  });

  it('reports a failed download', async () => {
    set('error', { error: 'Connection lost at 63 %.' });
    render(UpdateDialog);
    expect(screen.getByRole('alertdialog', { name: 'The download didn’t finish' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Connection lost at 63 %.');
    await click('Download from website');
    expect(mock.api.openUrl).toHaveBeenCalled();
    await click('Try again');
    expect(mock.api.updateInstall).toHaveBeenCalled();
  });

  it('falls back to the first stage for an unexpected one', () => {
    store.info = null;
    set('idle');
    render(UpdateDialog);
    expect(screen.getByRole('dialog', { name: 'Blockheads PC 0.2.0 is here' })).toBeVisible();
    cleanup();
    store.update = { stage: 'idle', info: null, progress: null, error: '', checked: false };
    render(UpdateDialog);
    expect(screen.getByRole('dialog')).toBeVisible();
  });
});
