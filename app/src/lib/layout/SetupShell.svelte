<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from '../i18n.svelte';
  import { SETUP_STEPS, store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';

  interface Props {
    /** Text at the bottom of the rail. */
    note: string;
    children: Snippet;
    /** Replaces the default Next button. */
    actions?: Snippet;
    /** Hide the status text next to Back (the ready step says "Ready"). */
    ready?: boolean;
  }
  let { note, children, actions, ready = false }: Props = $props();
  const NAMES = [
    ['setup.s1', 'setup.s1.sub'],
    ['setup.s2', 'setup.s2.sub'],
    ['setup.s3', 'setup.s3.sub'],
    ['setup.s4', 'setup.s4.sub'],
    ['setup.s5', 'setup.s5.sub'],
    ['setup.s6', 'setup.s6.sub']
  ] as const;
  const steps = $derived(
    Array.from({ length: SETUP_STEPS }, (_, i) => {
      const n = i + 1;
      return {
        n,
        label: t(NAMES[i][0]),
        sub: ready && n === SETUP_STEPS ? t('setup.allSet') : t(NAMES[i][1]),
        done: ready || n < store.step,
        cur: !ready && n === store.step
      };
    })
  );
</script>

<div class="win">
  <nav class="rail sunburst" aria-label={t('setup.stepsLabel')}>
    <div class="rb">
      <img src="/pc-icon-small.svg" alt="" width="32" height="32" />
      <span class="rn">Blockheads<span class="tg">{t('app.tag')}</span></span>
    </div>
    <ol class="steps">
      {#each steps as s (s.n)}
        <li aria-current={s.cur ? 'step' : undefined}>
          {#if s.done && !(ready && s.n === SETUP_STEPS)}
            <button class="st done" title={s.label} onclick={() => store.jump(s.n)}>
              <span class="n">✓</span><span class="lb">{s.label}<small>{s.sub}</small></span>
            </button>
          {:else}
            <div class={['st', s.cur && 'cur', s.done && 'done']} title={s.label}>
              <span class="n">{s.done ? '✓' : s.n}</span><span class="lb">{s.label}<small>{s.sub}</small></span>
            </div>
          {/if}
        </li>
      {/each}
    </ol>
    <p class="rfoot">{note}</p>
  </nav>

  <div class="rmain">
    <main class="wbody" id="main" tabindex="-1">{@render children()}</main>
    <footer class="wfoot">
      <Button icon="chev-l" disabled={store.step === 1} onclick={() => store.back()}>{t('common.back')}</Button>
      <span class="hint stat">
        {#if ready}<Icon name="check" size={16} />{t('setup.stepReady', { n: store.step, total: SETUP_STEPS })}
        {:else}{t('setup.stepOf', { n: store.step, total: SETUP_STEPS })}{/if}
      </span>
      {#if actions}
        {@render actions()}
      {:else}
        <Button variant="primary" size="lg" disabled={!store.canNext} onclick={() => store.next()}>
          {t('common.next')}<Icon name="chev-r" />
        </Button>
      {/if}
    </footer>
  </div>
</div>

<style>
  /* Fixed-height app layout: the rail stays put, only the step content scrolls (not the whole page). */
  .win { height: 100%; display: flex; overflow: hidden; background: var(--bg); }
  .rail {
    --ray: rgba(255, 122, 69, 0.12);
    flex: 1 1 230px;
    max-width: 280px;
    background-image: repeating-conic-gradient(from -3deg at 0% 112%, var(--ray) 0deg 7deg, transparent 7deg 22deg);
    color: var(--stage-text);
    padding: 22px 14px;
    display: flex;
    flex-direction: column;
    gap: 20px;
    overflow-y: auto;
  }
  .rb { display: flex; gap: 10px; align-items: center; padding: 0 6px; font: 800 18px/1.1 var(--f-display); letter-spacing: -0.01em; }
  .tg {
    display: inline-block;
    background: var(--play-bg);
    color: #1f1a17;
    padding: 2px 6px 3px;
    border-radius: 5px;
    transform: rotate(-6deg);
    box-shadow: 0 2px 0 var(--play-edge);
    font-size: 15px;
    margin-left: 4px;
  }
  .steps { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .st {
    display: flex;
    gap: 12px;
    align-items: center;
    min-height: 48px;
    padding: 4px 10px;
    border-radius: 10px;
    color: var(--stage-text-2);
    font-weight: 700;
    font-size: 15px;
    width: 100%;
    border: 0;
    background: transparent;
    text-align: left;
    font-family: inherit;
  }
  button.st { cursor: pointer; }
  button.st:hover { background: rgba(255, 255, 255, 0.06); }
  button.st:focus-visible { outline: 3px solid #86b9ff; outline-offset: 2px; }
  .n {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    border: 2px solid #54483f;
    font: 800 13px var(--f-display);
    flex: none;
  }
  .lb small { display: block; font-weight: 400; font-size: 12.5px; color: var(--stage-text-3); }
  .cur { background: rgba(255, 255, 255, 0.08); color: #fff; }
  .cur .n { background: #ff7a45; border-color: #ff7a45; color: #1c0d06; box-shadow: 0 2px 0 #9c3a12; }
  .done .n { background: #163a29; border-color: #56c990; color: #56c990; }
  .rfoot { margin-top: auto; font-size: 12.5px; color: var(--stage-text-3); padding: 0 6px; }
  .rmain { position: relative; flex: 999 1 520px; min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow-y: auto; }
  .wbody { flex: 1; padding: 32px 40px; display: flex; flex-direction: column; gap: 18px; max-width: 820px; width: 100%; outline: none; }
  .wfoot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: 14px 40px;
    border-top: 2px solid var(--line);
    background: var(--surface);
    flex-wrap: wrap;
    position: sticky;
    bottom: 0;
  }
  .stat { display: flex; gap: 6px; align-items: center; }
  .stat :global(.ico) { color: var(--ok); }
  @media (max-width: 900px) {
    .rail { flex: 0 0 76px; padding: 18px 10px; }
    .rn, .lb, .rfoot { display: none; }
    .rb { justify-content: center; padding: 0; }
    .st { justify-content: center; padding: 4px; }
    .wbody { padding: 24px; }
    .wfoot { padding: 12px 24px; }
  }
</style>
