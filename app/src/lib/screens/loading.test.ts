import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mock = vi.hoisted(() => ({ api: {} as Record<string, any>, win: {} as Record<string, any> }));
vi.mock('../api', () => ({
  api: new Proxy({}, { get: (_t, k: string) => mock.api[k] }),
  win: new Proxy({}, { get: (_t, k: string) => mock.win[k] })
}));

import { etaText } from '../format';
import { setLang } from '../i18n.svelte';
import { makeApi, makeSnap, makeWin } from '../mock-api';
import LoadingView from '../parts/LoadingView.svelte';
import { store } from '../store.svelte';
import { resetStore } from '../test-utils';
import type { BootProgress } from '../types';
import Playing from './Playing.svelte';

const prog = (over: Partial<BootProgress> = {}): BootProgress => ({
  percent: 64, phase: 'runtime', step: 4, elapsed_secs: 6, eta_secs: 12, slow: false, stalled: false,
  step_secs: [0.8, 2.4, 3.1, null, null, null], ready: false, ...over
});

const handlers = () => ({ oncancel: vi.fn(), onretry: vi.fn(), oncopylog: vi.fn(), onhome: vi.fn() });

beforeEach(() => {
  mock.api = makeApi();
  mock.win = makeWin().win;
  resetStore();
  setLang('en');
  vi.useFakeTimers({ shouldAdvanceTime: true });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('etaText', () => {
  it('rounds to 1 s under 10 s, to 5 s above and caps at about a minute', () => {
    expect(etaText(0.2)).toBe('about 1 s left');
    expect(etaText(7.4)).toBe('about 7 s left');
    expect(etaText(12)).toBe('about 10 s left');
    expect(etaText(13)).toBe('about 15 s left');
    expect(etaText(58)).toBe('about 1 min left');
    expect(etaText(57.4)).toBe('about 55 s left');
    expect(etaText(57.5)).toBe('about 1 min left');
    expect(etaText(300)).toBe('about 1 min left');
  });
});

describe('LoadingView', () => {
  it('shows the step, the percentage, the time left and the step list', () => {
    render(LoadingView, { progress: prog(), version: '1.7.6', ...handlers() });
    const bar = screen.getByRole('progressbar', { name: 'Starting the game' });
    expect(bar).toHaveAttribute('aria-valuenow', '64');
    expect(bar).toHaveAttribute('aria-valuetext', '64 percent, Preparing Android layer, about 10 s left');
    expect(screen.getByText('64 %')).toBeVisible();
    expect(screen.getByText('Step 4 of 6')).toBeVisible();
    expect(screen.getByText('about 10 s left')).toBeVisible();
    expect(screen.getByText('1.7.6')).toBeVisible();
    const rows = screen.getAllByRole('listitem');
    expect(rows).toHaveLength(6);
    expect(rows[3]).toHaveAttribute('aria-current', 'step');
    expect(rows[0]).toHaveTextContent('Reading game file0.8 s');
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  });

  it('estimates before there is a number, floors the percentage and has no version badge without one', () => {
    render(LoadingView, { progress: prog({ percent: 9.9, eta_secs: null, step: 1 }), version: '', ...handlers() });
    expect(screen.getByText('9 %')).toBeVisible();
    expect(screen.getByText('Estimating time left…')).toBeVisible();
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuetext', '9 percent, Reading game file, Estimating time left…');
  });

  it('copes with no report yet and out-of-range values', () => {
    const { rerender } = render(LoadingView, { progress: null, version: '', ...handlers() });
    expect(screen.getByText('0 %')).toBeVisible();
    expect(screen.getByText('Step 1 of 6')).toBeVisible();
    void rerender({ progress: prog({ percent: 250, step: 9, eta_secs: 3 }), version: '', ...handlers() });
  });

  it('says it is still working and explains a slow start', () => {
    render(LoadingView, { progress: prog({ slow: true, step: 5, percent: 81 }), version: '1.7.6', ...handlers() });
    expect(screen.getByText('Still working…')).toBeVisible();
    expect(screen.getByRole('status')).toHaveTextContent('This is taking longer than usual.');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuetext', '81 percent, Launching the game, Still working…');
  });

  it('shows the error with the step it stopped at, and its actions', async () => {
    const h = handlers();
    render(LoadingView, { progress: prog({ step: 3 }), version: '1.7.6', failed: true, logTail: ['a', 'b'], ...h });
    expect(screen.getByRole('alert')).toHaveTextContent('The game couldn’t start');
    expect(screen.getByRole('alert')).toHaveTextContent('It stopped at Starting game code.');
    expect(screen.getByLabelText('Last log lines')).toHaveTextContent('a b');
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(screen.getByRole('button', { name: 'Try again' })).toHaveFocus();
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Copy log' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Back to Home' }));
    expect([h.onretry, h.oncopylog, h.onhome].map((f) => f.mock.calls.length)).toEqual([1, 1, 1]);
  });

  it('explains a stall and works without a log', () => {
    render(LoadingView, { progress: prog({ stalled: true, step: 6 }), version: '', failed: true, ...handlers() });
    expect(screen.getByRole('alert')).toHaveTextContent('did not respond for a minute');
    expect(screen.queryByLabelText('Last log lines')).toBeNull();
    cleanup();
    render(LoadingView, { progress: null, version: '', failed: true, ...handlers() });
    expect(screen.getByRole('alert')).toHaveTextContent('A log was saved.');
  });

  it('hands over: full bar, all steps done, no Cancel', () => {
    render(LoadingView, { progress: prog({ ready: true }), version: '1.7.6', handover: true, ...handlers() });
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuetext', '100 percent, Waiting for the first frame');
    expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();
    expect(screen.getByText('The game has started.')).toBeInTheDocument();
    expect(screen.getByText('Step 6 of 6')).toBeVisible();
  });

  it('keeps Cancel reachable in fullscreen', async () => {
    const h = handlers();
    const { rerender } = render(LoadingView, { progress: prog(), version: '', fullscreen: true, ...h });
    const chip = screen.getAllByRole('button', { name: 'Cancel' });
    expect(chip).toHaveLength(2);
    await fireEvent.click(chip[1]);
    expect(h.oncancel).toHaveBeenCalled();
    await rerender({ progress: prog(), version: '', fullscreen: true, handover: true, ...h });
    expect(screen.queryAllByRole('button', { name: 'Cancel' })).toHaveLength(0);
  });

  it('speaks Turkish', () => {
    setLang('tr');
    render(LoadingView, { progress: prog({ eta_secs: 30 }), version: '', ...handlers() });
    expect(screen.getByText('Adım 4 / 6')).toBeVisible();
    expect(screen.getByText('yaklaşık 30 sn kaldı')).toBeVisible();
    expect(screen.getByText('Android katmanı hazırlanıyor', { selector: 'strong' })).toBeVisible();
  });
});

describe('Playing while the game boots', () => {
  const bar = () => screen.queryByRole('progressbar');

  it('shows nothing for a start under 400 ms, then the loader, then fades into the game', async () => {
    store.screen = 'playing';
    store.game = makeSnap({ state: 'starting', running_secs: 0, progress: prog() });
    const { unmount } = render(Playing);
    expect(bar()).toBeNull();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeVisible();
    // Fast: the first frame comes before the loader would show.
    store.game = makeSnap({ state: 'running', running_secs: 1, progress: prog({ ready: true, percent: 100 }) });
    await vi.advanceTimersByTimeAsync(600);
    expect(bar()).toBeNull();
    expect(screen.getByRole('button', { name: 'Stop game' })).toBeVisible();
    unmount();
    cleanup();

    // Slow: the loader shows after 400 ms, hands over on ready and is gone 240 ms later.
    store.game = makeSnap({ state: 'starting', running_secs: 0, progress: prog() });
    render(Playing);
    await vi.advanceTimersByTimeAsync(450);
    expect(bar()).not.toBeNull();
    expect(screen.getByRole('status', { name: 'Game status: Starting…' })).toBeVisible();
    store.game = makeSnap({ state: 'running', running_secs: 5, progress: prog({ ready: true, percent: 100 }) });
    await vi.advanceTimersByTimeAsync(50);
    expect(bar()).toHaveAttribute('aria-valuenow', '100');
    await vi.advanceTimersByTimeAsync(300);
    expect(bar()).toBeNull();
    expect(document.getElementById('main')).toHaveFocus();
  });

  it('stops on Cancel and on Esc, but not for Esc under a dialog or after the start', async () => {
    store.screen = 'playing';
    store.game = makeSnap({ state: 'starting', running_secs: 0, progress: prog() });
    render(Playing);
    await fireEvent.click(screen.getAllByRole('button', { name: 'Cancel' })[0]);
    expect(mock.api.gameStop).toHaveBeenCalledTimes(1);
    store.game = makeSnap({ state: 'starting', running_secs: 0, progress: prog() });
    store.dialog = 'logs';
    await fireEvent.keyDown(window, { key: 'Escape' });
    store.dialog = null;
    await fireEvent.keyDown(window, { key: 'x' });
    expect(mock.api.gameStop).toHaveBeenCalledTimes(1);
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(mock.api.gameStop).toHaveBeenCalledTimes(2);
    store.game = makeSnap({ state: 'running', progress: prog({ ready: true }) });
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(mock.api.gameStop).toHaveBeenCalledTimes(2);
  });

  it('has its own Cancel once the loader shows', async () => {
    store.screen = 'playing';
    store.game = makeSnap({ state: 'starting', running_secs: 0, progress: prog() });
    render(Playing);
    await vi.advanceTimersByTimeAsync(450);
    const cancels = screen.getAllByRole('button', { name: 'Cancel' });
    expect(cancels).toHaveLength(2);
    await fireEvent.click(cancels[1]);
    expect(mock.api.gameStop).toHaveBeenCalledTimes(1);
  });

  it('polls faster while booting and slows down again', async () => {
    const polls = vi.fn(async () => makeSnap({ state: 'starting', running_secs: 0, progress: prog() }));
    mock.api.gamePoll = polls;
    store.screen = 'home';
    await store.poll();
    expect(polls).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(polls.mock.calls.length).toBeGreaterThanOrEqual(4);
    await store.poll();
    mock.api.gamePoll = vi.fn(async () => makeSnap({ state: 'running', running_secs: 1, progress: prog({ ready: true }) }));
    await store.poll();
    const n = (mock.api.gamePoll as ReturnType<typeof vi.fn>).mock.calls.length;
    await vi.advanceTimersByTimeAsync(1000);
    expect((mock.api.gamePoll as ReturnType<typeof vi.fn>).mock.calls.length).toBe(n);
    store.stopPolling();
    await store.poll();
    store.stopPolling();
  });

  it('shows a failed start with "Didn’t start" and lets the user try again or go home', async () => {
    store.screen = 'playing';
    store.log = ['x', 'y', 'z', 'last'];
    store.game = makeSnap({ state: 'crashed', exit_code: 139, progress: prog({ step: 3, ready: false }) });
    render(Playing);
    expect(screen.getByRole('status', { name: 'Game status: Didn’t start' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Starting game code');
    expect(screen.getByLabelText('Last log lines')).toHaveTextContent('y z last');
    expect(screen.queryByRole('button', { name: 'Stop game' })).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Copy log' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Back to Home' }));
    expect(store.screen).toBe('home');
    store.screen = 'playing';
    await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(mock.api.gameStart).toHaveBeenCalled();
  });
});
