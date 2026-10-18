<script lang="ts">
  import { t } from '../i18n.svelte';
  import type { SplashState } from '../types';
  import Button from '../ui/Button.svelte';

  /** The whole content of the splash window (600x380 card; `variant` setup = yellow first-launch splash). */
  let { state, square = false, onretry, onopenlog, onquit }: {
    state: SplashState;
    /** No transparency (macOS): the window itself is the card, so no rounded corners. */
    square?: boolean;
    onretry: () => void;
    onopenlog: () => void;
    onquit: () => void;
  } = $props();
  const setup = $derived(state.first_run);
</script>

<div class="wrap">
  <section
    class={['sp', setup ? 'sp-s' : 'sp-l', square && 'sq']}
    role="status"
    aria-live="polite"
    aria-label={setup ? t('splash.setupLabel') : t('splash.label')}
  >
    <img class="ic" src={setup ? '/pc-icon-macos.svg' : '/pc-icon.svg'} alt="" width={setup ? 132 : 128} height={setup ? 132 : 128} />
    <div class="wm">Blockheads<span class="tg">PC</span></div>
    {#if setup}<p class="welcome">{t('splash.welcome')}</p>{/if}
    {#if state.boot === 'error'}
      <p class="err" role="alert">{setup ? t('splash.setupError') : t('splash.error')}</p>
      <p class="hint detail">{state.error}</p>
      <div class="acts">
        <Button variant="primary" onclick={() => onretry()}>{t('common.retry')}</Button>
        <Button onclick={() => onopenlog()}>{t('splash.openLog')}</Button>
        <Button variant="ghost" onclick={() => onquit()}>{t('common.quit')}</Button>
      </div>
    {:else}
      <div class="bar"><span></span></div>
      <p class="msg">{state.boot === 'slow' ? t(setup ? 'splash.slowSetup' : 'splash.slow') : t(setup ? 'splash.loadingSetup' : 'splash.loading')}</p>
    {/if}
    <div class="foot"><span>{t('splash.footer')}</span><span class="mono">{state.version}</span></div>
  </section>
</div>

<style>
  .wrap { height: 100vh; }
  .sp {
    width: 100%;
    height: 100%;
    border-radius: 18px;
    box-sizing: border-box;
    overflow: hidden;
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    text-align: center;
    padding: 24px 24px 48px;
  }
  .sq { border-radius: 0; }
  .sp-l { background-color: #1f1a17; background-image: repeating-conic-gradient(from -3deg at 50% 118%, rgba(255, 122, 69, 0.13) 0deg 7deg, transparent 7deg 22deg); color: #f5ede5; }
  .sp-s { background-color: #ffd27a; background-image: repeating-conic-gradient(from -3deg at 50% 118%, rgba(255, 255, 255, 0.4) 0deg 7deg, transparent 7deg 22deg); color: #1f1a17; border: 3px solid #1f1a17; }
  .ic { animation: drop 0.6s cubic-bezier(0.2, 0.9, 0.3, 1.2) both; }
  .sp-l .ic { filter: drop-shadow(0 8px 0 rgba(0, 0, 0, 0.4)); }
  @keyframes drop { from { transform: translateY(-24px) scale(0.9); opacity: 0; } }
  .wm { display: flex; align-items: center; gap: 8px; font: 800 30px/1 var(--f-display); letter-spacing: -0.02em; }
  .tg { display: inline-block; padding: 3px 8px 4px; border-radius: 7px; transform: rotate(-6deg); font-size: 24px; }
  .sp-l .tg { background: #ffd27a; color: #1f1a17; box-shadow: 0 3px 0 #c98a2e; }
  .sp-s .tg { background: #1f1a17; color: #ffd27a; box-shadow: 0 3px 0 #000; }
  .welcome { font-size: 17px; font-weight: 700; }
  .bar { width: 260px; max-width: 100%; height: 8px; border-radius: 999px; overflow: hidden; position: relative; margin-top: 6px; }
  .sp-l .bar { background: #3a312b; }
  .sp-s .bar { background: rgba(31, 26, 23, 0.18); }
  .bar span { position: absolute; top: 0; bottom: 0; width: 35%; border-radius: 999px; animation: slide 1.3s ease-in-out infinite; }
  .sp-l .bar span { background: #ff7a45; }
  .sp-s .bar span { background: #1f1a17; }
  @keyframes slide { from { left: -35%; } to { left: 100%; } }
  .msg { font-size: 14px; }
  .sp-l .msg { color: #cbbeb2; }
  .sp-s .msg { color: #3d2f22; }
  .err { font-size: 14.5px; font-weight: 700; max-width: 440px; }
  .sp-l .err { color: #ffd2cb; }
  .sp-s .err { color: #7a1a10; }
  .detail { max-width: 440px; overflow-wrap: anywhere; }
  .sp-l .detail { color: #ae9f92; }
  .sp-s .detail { color: #3d2f22; }
  .acts { display: flex; gap: 8px; flex-wrap: wrap; justify-content: center; }
  .foot { position: absolute; left: 18px; right: 18px; bottom: 14px; display: flex; justify-content: space-between; font-size: 12.5px; }
  .sp-l .foot { color: #ae9f92; }
  .sp-s .foot { color: #3d2f22; }
</style>
