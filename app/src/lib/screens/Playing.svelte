<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import LoadingView from '../parts/LoadingView.svelte';
  import BindingChips from '../ui/BindingChips.svelte';
  import Icon from '../ui/Icon.svelte';
  import StatusPill from '../ui/StatusPill.svelte';

  const minutes = $derived(Math.floor((store.game.running_secs ?? 0) / 60));
  const keys = $derived(store.config?.keys);

  /** The game stage: with the game inside this window the backend draws a native view over it. */
  let view = $state<HTMLElement>();
  const embedded = $derived(store.embedded);
  const hideBars = $derived(embedded && store.fullscreen);

  /** Once the game is ready the loader holds at 100 % this long, then hands over. */
  const HOLD_MS = 150;
  /** The loader fades out over this long (the CSS transition has the same length); the game fades in meanwhile. */
  const HANDOVER_MS = 150;
  const starting = $derived(store.starting);
  const failed = $derived(store.bootFailed);
  const ready = $derived(store.state === 'running' && !!store.game.progress?.ready);
  let shown = $state(false);
  let complete = $state(false);
  let handover = $state(false);
  const loader = $derived((shown && starting) || failed || complete || handover);
  const logTail = $derived(store.log.slice(-3));

  $effect(() => {
    // The loader shows at once: a boot takes seconds, and without it the empty (black) stage shows.
    if (starting) shown = true;
    else if (!ready) shown = false;
  });
  // First frame: the loader shows 100 %, holds briefly, then fades out while the in-window game (held back until
  // now, see `embed::reveal`) fades in; the game takes the focus.
  $effect(() => {
    if (!ready) {
      complete = false;
      handover = false;
      return;
    }
    if (!shown) {
      void api.embedReveal().catch(() => {});
      return;
    }
    complete = true;
    const fade = setTimeout(() => {
      handover = true;
      void api.embedReveal().catch(() => {});
    }, HOLD_MS);
    const done = setTimeout(() => {
      complete = false;
      handover = false;
      shown = false;
      view?.focus();
    }, HOLD_MS + HANDOVER_MS);
    return () => {
      clearTimeout(fade);
      clearTimeout(done);
    };
  });

  function onkey(e: KeyboardEvent) {
    if (e.key === 'Escape' && starting && store.dialog === null) void store.stop();
  }

  // Tell the backend where the stage starts: 0 when the bars are hidden (fullscreen), -1 while a dialog needs the
  // whole window (the game view steps aside).
  $effect(() => {
    const el = view;
    if (!embedded || !el) return;
    const dialog = store.dialog !== null;
    void store.fullscreen;
    void tick().then(() => api.embedLayout(dialog ? -1 : Math.round(el.getBoundingClientRect().top)).catch(() => {}));
  });
  // Leaving the screen (Home) hides the game view; it comes back with the next layout.
  onMount(() => () => void api.embedLayout(-1).catch(() => {}));
</script>

<svelte:window onkeydown={onkey} />

<div class="wrap">
  {#if !hideBars}
  <nav class="strip" aria-label={t('play.controls')}>
    <button class="sb" title={t('play.homeTitle')} onclick={() => store.go('home')}><Icon name="home" />{t('play.home')}</button>
    <StatusPill state={failed ? 'crashed' : store.game.state} label={failed ? t('status.failed') : undefined} {minutes} />
    <span class="sp"></span>
    {#if embedded}<button class="sb ic" aria-label={t('fs.enter')} title={t('fs.enter')} onclick={() => store.toggleFullscreen()}><Icon name="max" /></button>{/if}
    <button class="sb ic" aria-label={t('home.q.join')} onclick={() => (store.dialog = 'join')}><Icon name="help" /></button>
    <button class="sb ic" aria-label={t('home.q.logs')} onclick={() => (store.dialog = 'logs')}><Icon name="terminal" /></button>
    {#if starting}
      <button class="sb cancel" onclick={() => store.stop()}><Icon name="x" />{t('common.cancel')}</button>
    {:else if !failed}
      <button class="sb stop" onclick={() => store.stop()}><Icon name="stop" />{t('home.stop')}</button>
    {/if}
  </nav>
  {/if}
  <main class="view" id="main" tabindex="-1" aria-label={t('play.view')} bind:this={view}>
    {#if !embedded}
    <div class="game">
      <p class="t">{t('play.title')}</p>
      <p class="d">{t('play.body')}</p>
      {#if keys}
        <ul class="keys" aria-label={t('play.keys')}>
          <li><span>{t('act.touch')}</span><BindingChips binding={keys.touch} /></li>
          <li><span>{t('act.pan')}</span><BindingChips binding={keys.pan} /></li>
          <li><span>{t('act.zoom')}</span><BindingChips binding={keys.zoom} /></li>
          <li><span>{t('act.fullscreen')}</span><BindingChips binding={keys.fullscreen} /></li>
        </ul>
      {/if}
      <p class="d small">{t('play.focus')}</p>
    </div>
    {/if}
    {#if loader}
      <LoadingView
        progress={store.game.progress}
        version={store.version}
        {failed}
        {complete}
        {handover}
        fullscreen={hideBars}
        {logTail}
        oncancel={() => store.stop()}
        onretry={() => store.play()}
        oncopylog={() => store.copyLog()}
        onhome={() => store.go('home')}
      />
    {/if}
  </main>
</div>

<style>
  .wrap { height: 100%; overflow: hidden; display: flex; flex-direction: column; background: #0e0b09; color: var(--stage-text); }
  .strip { display: flex; align-items: center; gap: 6px; height: 44px; padding: 0 10px; background: #1f1a17; border-bottom: 1px solid #000; flex: none; }
  .sp { flex: 1; }
  .sb { display: inline-flex; align-items: center; gap: 8px; height: 34px; padding: 0 12px; border-radius: 9px; border: 0; background: transparent; color: #f5ede5; font: 700 14px var(--f-body); cursor: pointer; white-space: nowrap; }
  .sb:hover { background: rgba(255, 255, 255, 0.1); }
  .sb:focus-visible { outline: 3px solid #86b9ff; outline-offset: 2px; }
  .sb.ic { width: 34px; padding: 0; justify-content: center; }
  .sb.cancel { background: #3a312b; }
  .sb.stop { background: #ff8f80; color: #1c0d06; }
  .view { position: relative; flex: 1; min-height: 0; display: grid; place-items: center; background: #000; padding: 16px; outline: none; }
  .game {
    width: min(100%, 720px);
    background: repeating-linear-gradient(45deg, #16120f 0 14px, #1b1613 14px 28px);
    border: 2px dashed #3a312b;
    border-radius: 12px;
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    align-items: center;
    text-align: center;
  }
  .t { font: 750 22px var(--f-display); color: #cbbeb2; }
  .d { color: #8f8175; font-size: 14px; max-width: 520px; }
  .d.small { font-size: 13px; }
  .keys { list-style: none; margin: 4px 0; padding: 0; display: flex; flex-direction: column; gap: 8px; width: min(100%, 420px); }
  .keys li { display: flex; justify-content: space-between; gap: 12px; align-items: center; color: #cbbeb2; font-size: 14px; }
  .keys :global(.key) { background: #26201c; color: #f5ede5; border-color: #54483f; box-shadow: 0 3px 0 #000; }
  .keys :global(.mb) { background: #26201c; color: #f5ede5; border-color: #3a312b; }
</style>
