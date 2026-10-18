import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setLang } from '../i18n.svelte';
import Alert from './Alert.svelte';
import Badge from './Badge.svelte';
import BindingChips from './BindingChips.svelte';
import Button from './Button.svelte';
import Card from './Card.svelte';
import Dialog from './Dialog.svelte';
import Icon from './Icon.svelte';
import PlayButton from './PlayButton.svelte';
import Progress from './Progress.svelte';
import Row from './Row.svelte';
import Segmented from './Segmented.svelte';
import SelectField from './SelectField.svelte';
import Spinner from './Spinner.svelte';
import StatusPill from './StatusPill.svelte';
import Toasts from './Toasts.svelte';
import Toggle from './Toggle.svelte';
import { dismiss, toast, toasts } from './toast.svelte';

const text = (s: string) => createRawSnippet(() => ({ render: () => `<span>${s}</span>` }));

beforeEach(() => {
  setLang('en');
  toasts.items = [];
});
afterEach(() => cleanup());

describe('Button', () => {
  it('renders variants and handles clicks', async () => {
    const onclick = vi.fn();
    render(Button, { children: text('Go'), onclick, icon: 'check', variant: 'primary', size: 'sm', pressed: true, expanded: false, haspopup: true, title: 'tip' });
    const b = screen.getByRole('button', { name: 'Go' });
    await fireEvent.click(b);
    expect(onclick).toHaveBeenCalled();
    expect(b).toHaveAttribute('aria-pressed', 'true');
    expect(b).toHaveAttribute('aria-haspopup', 'menu');
    expect(b).toHaveAttribute('title', 'tip');
  });

  it('is icon-only without children, busy while loading, and a link with href', () => {
    const { unmount } = render(Button, { icon: 'x', label: 'Close', size: 'lg', block: true });
    expect(screen.getByRole('button', { name: 'Close' })).toHaveClass('icon-only');
    unmount();
    render(Button, { children: text('Wait'), loading: true });
    expect(screen.getByRole('button', { name: 'Wait' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Wait' })).toHaveAttribute('aria-busy', 'true');
    cleanup();
    render(Button, { children: text('Docs'), href: 'https://x', icon: 'ext', size: 'sm' });
    expect(screen.getByRole('link', { name: 'Docs' })).toHaveAttribute('href', 'https://x');
    cleanup();
    render(Button, { icon: 'ext', label: 'Open', href: 'https://y', block: true, variant: 'ghost' });
    expect(screen.getByRole('link', { name: 'Open' })).toHaveClass('icon-only', 'block');
    cleanup();
    render(Button, { children: text('Plain link'), href: 'https://z' });
    expect(screen.getByRole('link', { name: 'Plain link' })).toBeVisible();
    cleanup();
    render(Button, { children: text('Off'), disabled: true, variant: 'danger' });
    expect(screen.getByRole('button', { name: 'Off' })).toBeDisabled();
  });
});

describe('Toggle', () => {
  it('reports changes and honours disabled and hidden labels', async () => {
    const onchange = vi.fn();
    render(Toggle, { label: 'VSync', hideLabel: true, onchange });
    const sw = screen.getByRole('switch', { name: 'VSync' });
    await fireEvent.click(sw);
    expect(onchange).toHaveBeenCalledWith(true);
    cleanup();
    render(Toggle, { label: 'Sound', disabled: true, checked: true });
    expect(screen.getByRole('switch', { name: 'Sound' })).toBeDisabled();
    expect(screen.getByRole('switch', { name: 'Sound' })).toBeChecked();
  });
});

describe('Dialog', () => {
  it('traps focus, closes on Escape and on the scrim, and gives focus back', async () => {
    const opener = document.createElement('button');
    document.body.append(opener);
    opener.focus();
    const onclose = vi.fn();
    const { unmount } = render(Dialog, {
      title: 'Hello',
      description: 'About it',
      icon: 'alert',
      tone: 'bad',
      onclose,
      children: createRawSnippet(() => ({ render: () => '<div><button data-autofocus>First</button><button>Second</button></div>' })),
      footer: text('foot')
    });
    const dlg = screen.getByRole('dialog', { name: 'Hello' });
    await waitFor(() => expect(screen.getByText('First')).toHaveFocus());
    expect(screen.getByText('About it')).toBeVisible();
    const close = screen.getByRole('button', { name: 'Close' });
    close.focus();
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab', shiftKey: true });
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab' });
    screen.getByText('Second').focus();
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab' });
    expect(close).toHaveFocus();
    screen.getByText('First').focus();
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab' });
    screen.getByText('Second').focus();
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab', shiftKey: true });
    await fireEvent.keyDown(dlg.parentElement!, { key: 'a' });
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Escape' });
    await fireEvent.mouseDown(dlg);
    await fireEvent.mouseDown(dlg.parentElement!);
    expect(onclose).toHaveBeenCalledTimes(2);
    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  it('works without icon, body or footer, wide and as an alert dialog', () => {
    render(Dialog, { title: 'Plain', onclose: () => {}, wide: true, alert: true, icon: 'info' });
    expect(screen.getByRole('alertdialog', { name: 'Plain' })).toHaveClass('wide');
  });

  it('keeps Tab inside when only one control exists', async () => {
    render(Dialog, { title: 'One', onclose: () => {} });
    const dlg = screen.getByRole('dialog');
    const close = screen.getByRole('button', { name: 'Close' });
    await waitFor(() => expect(close).toHaveFocus());
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab' });
    await fireEvent.keyDown(dlg.parentElement!, { key: 'Tab', shiftKey: true });
    expect(close).toHaveFocus();
  });
});

describe('Toasts', () => {
  it('shows messages with tone and runs the action', async () => {
    vi.useFakeTimers();
    render(Toasts);
    const run = vi.fn();
    const a = toast('Saved', { label: 'Undo', run });
    toast('Failed', undefined, 'bad');
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Failed'));
    await fireEvent.click(await screen.findByRole('button', { name: 'Undo' }));
    expect(run).toHaveBeenCalled();
    expect(toasts.items.some((t) => t.id === a)).toBe(false);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(toasts.items).toHaveLength(0);
    dismiss(999);
    vi.useRealTimers();
  });
});

describe('Alert, Badge, Card, Row, Icon', () => {
  it('renders alert tones, actions and bodies', () => {
    const { unmount } = render(Alert, { tone: 'bad', title: 'Broke', children: text('why'), actions: text('act') });
    expect(screen.getByRole('alert')).toHaveTextContent('Broke');
    unmount();
    render(Alert, { title: 'FYI', tone: 'ok', icon: 'check' });
    expect(screen.getByRole('note')).toHaveTextContent('FYI');
    cleanup();
    render(Alert, { title: 'Hmm' });
    expect(screen.getByRole('note')).toBeVisible();
  });

  it('renders badges', () => {
    render(Badge, { tone: 'ok', icon: 'check', dot: true, pulse: true, label: 'status', children: text('Fine') });
    expect(screen.getByLabelText('status')).toHaveTextContent('Fine');
    cleanup();
    render(Badge, { children: text('Plain') });
    expect(screen.getByText('Plain')).toBeVisible();
  });

  it('renders cards and rows', () => {
    render(Card, { children: text('inside'), tone: 'ok', label: 'box', pad: false, class: 'x' });
    expect(screen.getByLabelText('box')).toHaveClass('ok', 'x');
    cleanup();
    render(Card, { children: text('plain') });
    expect(screen.getByText('plain')).toBeVisible();
    cleanup();
    render(Row, { title: 'Path', hint: '/x', mono: true, first: true, titleId: 't', children: text('ctl'), lead: text('lead') });
    expect(screen.getByText('Path')).toHaveAttribute('id', 't');
    cleanup();
    render(Row, { title: '' });
    expect(document.querySelector('.t')).toBeNull();
  });

  it('draws icons', () => {
    render(Icon, { name: 'play', size: 30 });
    expect(document.querySelector('svg')).toHaveAttribute('width', '30');
  });
});

describe('inputs', () => {
  it('selects an option and renders hints', async () => {
    const onchange = vi.fn();
    render(SelectField, {
      label: 'Language',
      value: 'en',
      options: [{ value: 'en', label: 'English', lang: 'en' }, { value: 'tr', label: 'Türkçe' }],
      hint: 'Pick one',
      onchange
    });
    await fireEvent.change(screen.getByLabelText('Language'), { target: { value: 'tr' } });
    expect(onchange).toHaveBeenCalledWith('tr');
    expect(screen.getByText('Pick one')).toBeVisible();
    cleanup();
    render(SelectField, { label: 'Hidden', value: 'a', disabled: true, hideLabel: true, options: [{ value: 'a', label: 'A' }], onchange });
    expect(screen.getByLabelText('Hidden')).toBeDisabled();
  });

  it('is a radio group with arrow keys', async () => {
    const onchange = vi.fn();
    render(Segmented, {
      label: 'Theme',
      value: 'light',
      options: [{ value: 'light', label: 'Light', icon: 'sun' }, { value: 'dark', label: 'Dark' }, { value: 'system', label: 'System' }],
      onchange
    });
    const light = screen.getByRole('radio', { name: 'Light' });
    expect(screen.getByRole('radiogroup', { name: 'Theme' })).toBeVisible();
    expect(light).toHaveAttribute('aria-checked', 'true');
    expect(light).toHaveAttribute('tabindex', '0');
    expect(screen.getByRole('radio', { name: 'Dark' })).toHaveAttribute('tabindex', '-1');
    await fireEvent.keyDown(light, { key: 'ArrowRight' });
    expect(onchange).toHaveBeenLastCalledWith('dark');
    await fireEvent.keyDown(light, { key: 'ArrowLeft' });
    expect(onchange).toHaveBeenLastCalledWith('system');
    await fireEvent.keyDown(light, { key: 'ArrowDown' });
    await fireEvent.keyDown(light, { key: 'ArrowUp' });
    await fireEvent.keyDown(light, { key: 'x' });
    expect(onchange).toHaveBeenCalledTimes(4);
    await fireEvent.click(screen.getByRole('radio', { name: 'Dark' }));
    expect(onchange).toHaveBeenLastCalledWith('dark');
  });
});

describe('progress, spinner and status', () => {
  it('renders determinate and indeterminate bars', () => {
    render(Progress, { label: 'Import', value: 40, valuetext: '40 percent' });
    const bar = screen.getByRole('progressbar', { name: 'Import' });
    expect(bar).toHaveAttribute('aria-valuenow', '40');
    expect(bar).toHaveAttribute('aria-valuetext', '40 percent');
    cleanup();
    render(Progress, { label: 'Wait', stage: true });
    expect(screen.getByRole('progressbar', { name: 'Wait' })).not.toHaveAttribute('aria-valuenow');
    cleanup();
    render(Spinner, { light: true, size: 16 });
    render(Spinner);
    expect(document.querySelectorAll('.spin')).toHaveLength(2);
  });

  it('names every game status', () => {
    for (const [state, label] of [
      ['ready', 'Ready to play'], ['launching', 'Starting'], ['running', 'Running · 12 min'],
      ['stopped', 'Stopped'], ['crashed', 'Crashed'], ['missing', 'Game file missing']
    ] as const) {
      const { unmount } = render(StatusPill, { state, minutes: 12 });
      expect(screen.getByRole('status', { name: `Game status: ${label}` })).toBeVisible();
      unmount();
    }
    render(StatusPill, { state: 'ready', live: false });
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('draws the Play button in its states', async () => {
    const onclick = vi.fn();
    render(PlayButton, { children: text('Play'), onclick });
    await fireEvent.click(screen.getByRole('button', { name: 'Play' }));
    expect(onclick).toHaveBeenCalled();
    cleanup();
    render(PlayButton, { children: text('Starting…'), busy: true, disabled: true, size: 'md', describedby: 'x', onclick });
    const b = screen.getByRole('button', { name: 'Starting…' });
    expect(b).toBeDisabled();
    expect(b).toHaveAttribute('aria-busy', 'true');
    expect(b).toHaveAttribute('aria-describedby', 'x');
  });

  it('draws bindings as key caps and mouse chips', () => {
    render(BindingChips, { binding: 'Ctrl+Left', conflict: true });
    expect(screen.getByText('Ctrl')).toHaveClass('bad');
    expect(screen.getByText('Left button')).toBeVisible();
    cleanup();
    render(BindingChips, { binding: 'Wheel' });
    expect(screen.getByText('Wheel')).toBeVisible();
    cleanup();
    render(BindingChips, { binding: 'Esc' });
    expect(screen.getByText('Esc')).not.toHaveClass('bad');
  });
});
