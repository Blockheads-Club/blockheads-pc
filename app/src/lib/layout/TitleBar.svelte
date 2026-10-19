<script lang="ts">
  import { onMount } from 'svelte';
  import { win } from '../api';
  import { t } from '../i18n.svelte';
  import type { Os } from '../types';

  interface Props {
    os: Os;
    /** The part after "Blockheads PC ·". */
    ctx?: string;
    /** The OS draws the buttons (macOS overlay title bar): leave room for them. */
    native?: boolean;
    /** Joined to the dark stage (while playing). */
    onStage?: boolean;
    /** Window not focused: grey lights, dim text. */
    inactive?: boolean;
    /** Force the maximized look (screenshots); otherwise tracked from the window. */
    maximized?: boolean;
  }
  let { os, ctx = '', native = false, onStage = false, inactive = false, maximized }: Props = $props();

  let isMax = $state(false);
  const max = $derived(maximized ?? isMax);

  onMount(() => {
    const sync = () => void win.isMaximized().then((m) => (isMax = m)).catch(() => {});
    sync();
    let off: (() => void) | undefined;
    void win.onResized(sync).then((u) => (off = u)).catch(() => {});
    return () => off?.();
  });

  const minimize = () => void win.minimize();
  const toggle = () => void win.toggleMaximize();
  const close = () => void win.close();
  const maxLabel = $derived(max ? t('tb.restore') : t('tb.maximize'));
</script>

<header
  class={['bar', os, onStage && 'stage', inactive && 'inactive']}
  aria-label={t('tb.label')}
  data-tauri-drag-region
>
  {#if os === 'macos' && !native}
    <div class="lights" role="group" aria-label={t('tb.controls')}>
      <button class="tl c" aria-label={t('tb.close')} onclick={close}></button>
      <button class="tl m" aria-label={t('tb.minimize')} onclick={minimize}></button>
      <button class="tl x" aria-label={maxLabel} onclick={toggle}></button>
    </div>
  {:else if os === 'macos'}
    <div class="native" aria-hidden="true" data-tauri-drag-region></div>
  {/if}
  <div class="title" data-tauri-drag-region>
    <img src="/pc-icon-small.svg" alt="" width="18" height="18" data-tauri-drag-region />
    <span class="name" data-tauri-drag-region>Blockheads <span class="tg">{t('app.tag')}</span></span>
    {#if ctx}<span class="ctx" data-tauri-drag-region>{`· ${ctx}`}</span>{/if}
  </div>
  {#if os === 'windows'}
    <div class="wctl" role="group" aria-label={t('tb.controls')}>
      <button class="w" aria-label={t('tb.minimize')} onclick={minimize}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" stroke="currentColor" stroke-width="1.2" /></svg>
      </button>
      <button class="w" aria-label={maxLabel} onclick={toggle}>
        {#if max}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x=".6" y="2.6" width="6.8" height="6.8" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" /><path d="M2.6 2.6V1.6A1 1 0 0 1 3.6.6h4.8a1 1 0 0 1 1 1v4.8a1 1 0 0 1-1 1h-1" fill="none" stroke="currentColor" stroke-width="1.2" /></svg>
        {:else}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x=".6" y=".6" width="8.8" height="8.8" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.2" /></svg>
        {/if}
      </button>
      <button class="w close" aria-label={t('tb.close')} onclick={close}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M.8.8l8.4 8.4M9.2.8L.8 9.2" stroke="currentColor" stroke-width="1.2" /></svg>
      </button>
    </div>
  {:else if os === 'linux'}
    <div class="lctl" role="group" aria-label={t('tb.controls')}>
      <button class="l" aria-label={t('tb.minimize')} onclick={minimize}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M1 7h8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
      </button>
      <button class="l" aria-label={maxLabel} onclick={toggle}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="1.3" y="1.3" width="7.4" height="7.4" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5" /></svg>
      </button>
      <button class="l close" aria-label={t('tb.close')} onclick={close}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
      </button>
    </div>
  {/if}
</header>

<style>
  .bar {
    --b-bg: var(--surface-2);
    --b-line: var(--line);
    --b-fg: var(--text);
    --b-fg2: var(--text-3);
    --b-hov: rgba(31, 26, 23, 0.08);
    --b-round: var(--surface-3);
    --b-round-hov: #dcd2c6;
    box-sizing: border-box;
    flex: none;
    width: 100%;
    height: var(--titlebar-h);
    display: flex;
    align-items: center;
    background: var(--b-bg);
    border-bottom: 1px solid var(--b-line);
    color: var(--b-fg);
    user-select: none;
    -webkit-user-select: none;
  }
  :global(:root[data-theme='dark']) .bar { --b-bg: #1d1815; --b-line: #3a312b; --b-fg: #f5ede5; --b-fg2: #ae9f92; --b-hov: rgba(255,255,255,.12); --b-round: #312924; --b-round-hov: #4a413c; }
  @media (prefers-color-scheme: dark) {
    :global(:root:not([data-theme='light']):not([data-theme='dark'])) .bar { --b-bg: #1d1815; --b-line: #3a312b; --b-fg: #f5ede5; --b-fg2: #ae9f92; --b-hov: rgba(255,255,255,.12); --b-round: #312924; --b-round-hov: #4a413c; }
  }
  .bar.stage { --b-bg: #1f1a17; --b-line: #000; --b-fg: #f5ede5; --b-fg2: #ae9f92; --b-hov: rgba(255,255,255,.12); --b-round: #312924; --b-round-hov: #4a413c; }
  .bar.inactive { --b-fg: var(--b-fg2); }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding-left: 12px;
    height: 100%;
  }
  .macos .title { justify-content: center; padding-right: 72px; }
  .title img { flex: none; pointer-events: none; }
  .inactive .title img { opacity: 0.55; }
  .name { font: 700 13.5px/1 var(--f-body); white-space: nowrap; color: var(--b-fg); }
  .tg { display: inline-block; margin-left: 2px; padding: 2px 6px 3px; border-radius: 6px; transform: rotate(-4deg); font-size: 12px; background: #ffd27a; color: #1f1a17; box-shadow: 0 2px 0 #c98a2e; }
  .ctx { font: 400 13px/1 var(--f-body); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--b-fg2); }
  button { border: 0; margin: 0; padding: 0; display: grid; place-items: center; cursor: default; font: inherit; color: var(--b-fg); background: transparent; }
  button:focus-visible { outline: 3px solid var(--focus); outline-offset: -3px; }
  .wctl { display: flex; height: 100%; flex: none; }
  .w { width: 46px; height: 100%; }
  .w:hover { background: var(--b-hov); }
  .w.close:hover { background: #c8301e; color: #fff; }
  .lctl { display: flex; gap: 10px; align-items: center; padding: 0 10px; flex: none; }
  .l { width: 28px; height: 28px; border-radius: 50%; background: var(--b-round); }
  .l:hover { background: var(--b-round-hov); }
  .l.close:hover { background: #c8301e; color: #fff; }
  .lights { display: flex; gap: 8px; align-items: center; padding: 0 6px 0 14px; flex: none; }
  .native { width: 78px; height: 100%; flex: none; }
  .tl { width: 12px; height: 12px; border-radius: 50%; box-shadow: inset 0 0 0 0.5px rgba(0, 0, 0, 0.2); }
  .tl.c { background: #ff5f57; }
  .tl.m { background: #febc2e; }
  .tl.x { background: #28c840; }
  .inactive .tl { background: #cfc1b1; }
  .tl:hover { filter: brightness(0.92); }
</style>
