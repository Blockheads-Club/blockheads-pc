<script lang="ts">
  import { onMount } from 'svelte';
  import { etaText } from '../format';
  import { t } from '../i18n.svelte';
  import type { Key } from '../messages';
  import type { BootProgress } from '../types';
  import Icon from '../ui/Icon.svelte';

  /**
   * The loading screen of the game area (design: PC-Loading). Everything it shows comes from its props, so the
   * plumbing (polling, hand-over timing, stopping) lives in the screen that uses it.
   */
  interface Props {
    /** What the runner reported; `null` before its first line. */
    progress?: BootProgress | null;
    version: string;
    /** The start failed: show the error with the step it stopped at. */
    failed?: boolean;
    /** The game is ready: the bar shows 100 % and every step done, before the hand-over. */
    complete?: boolean;
    /** The first frame is up: the loader fades out (instantly with reduced motion). */
    handover?: boolean;
    /** Fullscreen: no strip, so a small chip keeps Cancel reachable. */
    fullscreen?: boolean;
    /** Last log lines, shown in the error state. */
    logTail?: string[];
    oncancel: () => void;
    onretry: () => void;
    oncopylog: () => void;
    onhome: () => void;
  }
  let {
    progress, version, failed = false, complete = false, handover = false, fullscreen = false, logTail = [],
    oncancel, onretry, oncopylog, onhome
  }: Props = $props();

  const STEPS = 6;
  const label = (n: number) => t(`load.step${n}` as Key);
  const step = $derived(Math.min(STEPS, Math.max(1, progress?.step ?? 1)));
  const done = $derived(handover || complete);
  const percent = $derived(done ? 100 : Math.floor(Math.min(100, Math.max(0, progress?.percent ?? 0))));
  const slow = $derived(!failed && !done && !!progress?.slow);
  const current = $derived(done ? label(STEPS) : label(step));
  const eta = $derived.by(() => {
    if (done) return '';
    if (slow) return t('load.etaSlow');
    const secs = progress?.eta_secs;
    return secs === null || secs === undefined ? t('load.etaWait') : etaText(secs);
  });
  const valueText = $derived(
    eta && !done
      ? t('load.valueEta', { p: percent, step: current, eta })
      : t('load.valueText', { p: percent, step: current })
  );
  const rows = $derived(
    Array.from({ length: STEPS }, (_, i) => {
      const n = i + 1;
      const ok = done || n < step;
      const cur = !done && !failed && n === step;
      const fail = failed && n === step;
      const secs = progress?.step_secs?.[i];
      return {
        n, label: label(n), done: ok, cur, fail,
        cls: ok ? 'done' : cur ? (slow ? 'cur slow' : 'cur') : fail ? 'fail' : '',
        time: ok && typeof secs === 'number' ? `${secs.toFixed(1)} s` : ''
      };
    })
  );

  let cancel = $state<HTMLButtonElement>();
  let retry = $state<HTMLButtonElement>();
  // Focus starts on Cancel; after a failure it goes to "Try again".
  onMount(() => cancel?.focus());
  $effect(() => {
    if (failed) retry?.focus();
  });
</script>

<section class={['ld', handover && 'hand']} aria-labelledby="ldt">
  <div class="col">
    <img class="logo" src="/pc-icon.svg" alt="" width="96" height="96" />
    <h1 id="ldt">{t('home.game')} {#if version}<span class="ver">{version}</span>{/if}</h1>

    {#if !failed}
      <div class="pbox">
        <div class="prow">
          <span class="lbl"><strong>{current}</strong><small>{t('load.stepOf', { a: done ? STEPS : step, b: STEPS })}</small></span>
          <span class="num"><span class="pct">{`${percent} %`}</span><small class:warn={slow}>{`${eta}`}</small></span>
        </div>
        <div
          class={['bar', slow && 'slow', done && 'done']}
          role="progressbar"
          aria-label={t('load.title')}
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={percent}
          aria-valuetext={valueText}
        ><span style:width={`${percent}%`}></span></div>
      </div>
    {/if}

    {#if slow}
      <div class="note warnbox" role="status">
        <Icon name="clock" /><p><strong>{t('load.slowTitle')}</strong> {t('load.slowBody')}</p>
      </div>
    {/if}

    {#if failed}
      <div class="note badbox" role="alert">
        <p class="et"><Icon name="alert" size={22} />{t('load.errTitle')}</p>
        <p class="eb">{t('load.errStep')} <strong>{label(step)}</strong>. {progress?.stalled ? t('load.errStalled') : t('load.errBody')}</p>
        {#if logTail.length}<pre class="log" aria-label={t('load.logLabel')}>{logTail.join('\n')}</pre>{/if}
      </div>
    {/if}

    <ol class="steps" aria-label={t('load.steps')}>
      {#each rows as r (r.n)}
        <li class={['st', r.cls]} aria-current={r.cur || r.fail ? 'step' : undefined}>
          <span class="si" aria-hidden="true">
            {#if r.done}<Icon name="check" size={14} />{:else if r.cur}<span class="spin"></span>{:else if r.fail}<Icon name="x" size={14} />{:else}<Icon name="dot" size={14} />{/if}
          </span>
          <span>{r.label}</span><span class="tm">{r.time}</span>
        </li>
      {/each}
    </ol>

    <div class="actions">
      {#if failed}
        <button class="btn primary" bind:this={retry} onclick={onretry}><Icon name="refresh" />{t('common.retry')}</button>
        <button class="btn" onclick={oncopylog}><Icon name="copy" />{t('logs.copy')}</button>
        <button class="btn ghost" onclick={onhome}><Icon name="home" />{t('load.backHome')}</button>
      {:else if !handover}
        <button class="btn" bind:this={cancel} onclick={oncancel}><Icon name="x" />{t('common.cancel')}</button>
        <span class="esc"><kbd>Esc</kbd>{t('load.escHint')}</span>
      {/if}
    </div>
  </div>
  <p class="sr" aria-live="polite">{handover ? t('load.started') : failed ? t('load.errTitle') : current}</p>
</section>

{#if fullscreen && !failed && !handover}
  <div class="chip"><span>{t('status.starting')}</span><button class="chipbtn" onclick={oncancel}><Icon name="x" />{t('common.cancel')}</button></div>
{/if}

<style>
  .ld {
    --ray: rgba(226, 85, 58, 0.07);
    position: absolute; inset: 0; z-index: 2; overflow: auto; padding: 28px 24px;
    display: flex; align-items: center; justify-content: center;
    background-color: var(--bg);
    background-image: repeating-conic-gradient(from -3deg at 50% 125%, var(--ray) 0deg 7deg, transparent 7deg 22deg);
    color: var(--text);
    transition: opacity 150ms ease;
  }
  :global(:root[data-theme='dark']) .ld { --ray: rgba(255, 122, 69, 0.09); }
  .ld.hand { opacity: 0; pointer-events: none; }
  .col { width: 100%; max-width: 600px; display: flex; flex-direction: column; align-items: center; gap: 18px; text-align: center; }
  .logo { filter: drop-shadow(0 6px 0 rgba(0, 0, 0, 0.25)); }
  h1 { font: 800 32px/1.1 var(--f-display); letter-spacing: -0.015em; margin: 0; text-wrap: balance; }
  .ver { font: 600 15px var(--f-mono); padding: 3px 10px; border-radius: 8px; background: var(--surface-2); border: 2px solid var(--line); color: var(--text-2); vertical-align: middle; }
  .pbox { width: 100%; display: flex; flex-direction: column; gap: 8px; }
  .prow { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; flex-wrap: wrap; text-align: left; }
  .lbl, .num { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .num { align-items: flex-end; }
  .lbl strong { font-size: 16px; }
  small { font-size: 13.5px; color: var(--text-3); }
  small.warn { color: var(--warn); }
  .pct { font: 800 28px/1 var(--f-display); font-variant-numeric: tabular-nums; }
  .bar { height: 18px; border-radius: 999px; background: var(--surface-3); border: 2px solid var(--line-strong); overflow: hidden; box-shadow: var(--edge); }
  .bar > span { display: block; height: 100%; border-radius: 999px; background: var(--primary); transition: width 0.3s ease; }
  .bar.slow > span { background: repeating-linear-gradient(-45deg, var(--warn) 0 10px, color-mix(in oklab, var(--warn), #fff 25%) 10px 20px); }
  .bar.done > span { background: var(--ok); transition-duration: 0.1s; }
  .note { width: 100%; display: flex; gap: 12px; align-items: flex-start; padding: 12px 14px; border-radius: 12px; text-align: left; font-size: 14.5px; color: var(--text); }
  .note p { margin: 0; }
  .warnbox { background: var(--warn-soft); }
  .warnbox :global(svg) { color: var(--warn); margin-top: 2px; }
  .badbox { background: var(--bad-soft); flex-direction: column; gap: 8px; }
  .et { display: flex; gap: 10px; align-items: center; font: 800 20px var(--f-display); color: var(--text); }
  .et :global(svg) { color: var(--bad); }
  .eb { color: var(--text-2); }
  .eb strong { color: var(--text); }
  .log { width: 100%; background: var(--code-bg); color: #f5ede5; font: 12.5px/1.65 var(--f-mono); padding: 10px 12px; border-radius: 10px; margin: 0; white-space: pre-wrap; text-align: left; }
  .steps { width: 100%; list-style: none; margin: 0; padding: 6px 0; border-radius: 16px; background: var(--surface); border: 2px solid var(--line); text-align: left; }
  .st { display: flex; align-items: center; gap: 12px; min-height: 38px; padding: 4px 16px; font-size: 14.5px; color: var(--text-3); }
  .si { width: 24px; height: 24px; border-radius: 7px; display: grid; place-items: center; flex: none; background: var(--surface-2); }
  .st.done { color: var(--text-2); }
  .st.done .si { background: var(--ok-soft); color: var(--ok); }
  .st.cur { color: var(--text); font-weight: 700; background: var(--primary-soft); }
  .st.cur .si { background: var(--primary); color: var(--on-primary); }
  .st.slow { background: var(--warn-soft); }
  .st.slow .si { background: var(--warn); color: #1c0d06; }
  .st.fail { color: var(--text); font-weight: 700; background: var(--bad-soft); }
  .st.fail .si { background: var(--bad); color: var(--on-primary); }
  .tm { margin-left: auto; font: 400 12.5px var(--f-mono); color: var(--text-3); }
  .spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid rgba(255, 255, 255, 0.35); border-top-color: currentColor; animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .actions { display: flex; gap: 10px; flex-wrap: wrap; justify-content: center; align-items: center; }
  .btn { display: inline-flex; align-items: center; justify-content: center; gap: 8px; min-height: 44px; padding: 0 18px; border-radius: 10px; border: 2px solid var(--line-strong); background: var(--surface); color: var(--text); font: 700 15px/1.2 var(--f-body); box-shadow: var(--edge); cursor: pointer; white-space: nowrap; }
  .btn.primary { background: var(--primary); border-color: var(--primary-edge); color: var(--on-primary); box-shadow: 0 3px 0 var(--primary-edge); }
  .btn.ghost { background: transparent; border-color: transparent; box-shadow: none; color: var(--text-2); }
  .btn:focus-visible, .chipbtn:focus-visible { outline: 3px solid var(--focus); outline-offset: 2px; }
  .esc { display: flex; gap: 6px; align-items: center; font-size: 13px; color: var(--text-3); }
  kbd { font: 600 12px/1 var(--f-mono); padding: 3px 6px; border-radius: 5px; border: 1.5px solid var(--line-strong); color: var(--text-2); }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .chip { position: absolute; z-index: 3; top: 14px; right: 14px; display: flex; gap: 8px; align-items: center; padding: 6px 6px 6px 12px; border-radius: 999px; background: rgba(31, 26, 23, 0.9); border: 1px solid #54483f; color: #f5ede5; font-size: 13px; }
  .chipbtn { display: inline-flex; align-items: center; gap: 8px; height: 30px; padding: 0 12px; border-radius: 9px; border: 0; background: #3a312b; color: #f5ede5; font: 700 14px var(--f-body); cursor: pointer; }
  @media (max-height: 700px) { .steps { display: none; } .col { gap: 14px; } }
  @media (prefers-reduced-motion: reduce) {
    .ld, .bar > span { transition: none; }
    .spin { animation: none; }
  }
</style>
