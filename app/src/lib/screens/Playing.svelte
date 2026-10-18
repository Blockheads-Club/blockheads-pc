<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import BindingChips from '../ui/BindingChips.svelte';
  import Icon from '../ui/Icon.svelte';
  import StatusPill from '../ui/StatusPill.svelte';

  const minutes = $derived(Math.floor((store.game.running_secs ?? 0) / 60));
  const keys = $derived(store.config?.keys);
</script>

<div class="wrap">
  <nav class="strip" aria-label={t('play.controls')}>
    <button class="sb" title={t('play.homeTitle')} onclick={() => store.go('home')}><Icon name="home" />{t('play.home')}</button>
    <StatusPill state={store.game.state} {minutes} />
    <span class="sp"></span>
    <button class="sb ic" aria-label={t('home.q.join')} onclick={() => (store.dialog = 'join')}><Icon name="help" /></button>
    <button class="sb ic" aria-label={t('home.q.logs')} onclick={() => (store.dialog = 'logs')}><Icon name="terminal" /></button>
    <button class="sb stop" onclick={() => store.stop()}><Icon name="stop" />{t('home.stop')}</button>
  </nav>
  <main class="view" id="main" tabindex="-1" aria-label={t('play.view')}>
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
  </main>
</div>

<style>
  .wrap { min-height: 100%; display: flex; flex-direction: column; background: #0e0b09; color: var(--stage-text); }
  .strip { display: flex; align-items: center; gap: 6px; height: 44px; padding: 0 10px; background: #1f1a17; border-bottom: 1px solid #000; flex: none; }
  .sp { flex: 1; }
  .sb { display: inline-flex; align-items: center; gap: 8px; height: 34px; padding: 0 12px; border-radius: 9px; border: 0; background: transparent; color: #f5ede5; font: 700 14px var(--f-body); cursor: pointer; white-space: nowrap; }
  .sb:hover { background: rgba(255, 255, 255, 0.1); }
  .sb:focus-visible { outline: 3px solid #86b9ff; outline-offset: 2px; }
  .sb.ic { width: 34px; padding: 0; justify-content: center; }
  .sb.stop { background: #ff8f80; color: #1c0d06; }
  .view { flex: 1; min-height: 0; display: grid; place-items: center; background: #000; padding: 16px; outline: none; }
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
