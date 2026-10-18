<script lang="ts">
  import { t } from '../i18n.svelte';
  import type { GameState } from '../types';

  interface Props {
    state: GameState;
    /** Minutes of play time, shown on the running pill. */
    minutes?: number;
    /** Announce changes (the Home pill); the small copy on the strip does not. */
    live?: boolean;
  }
  let { state, minutes = 0, live = true }: Props = $props();
  const label = $derived(
    state === 'running' ? t('status.runningFor', { m: minutes }) : t(`status.${state}` as 'status.ready')
  );
  const pulse = $derived(state === 'launching' || state === 'running');
</script>

<span
  class={['pill', state]}
  role={live ? 'status' : undefined}
  aria-label={live ? t('status.aria', { s: label }) : undefined}
>
  <span class={['dot', pulse && 'pulse']}></span>{label}
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    border-radius: 999px;
    font: 700 13px/1.4 var(--f-body);
    white-space: nowrap;
  }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: currentColor; flex: none; }
  .pulse { animation: pulse 2s ease-in-out infinite; }
  @keyframes pulse { 50% { opacity: 0.35; } }
  .ready, .running { background: var(--stage-ok-bg); color: var(--stage-ok); }
  .launching, .missing { background: var(--stage-warn-bg); color: var(--stage-warn); }
  .stopped { background: var(--stage-neutral-bg); color: var(--stage-text-2); }
  .crashed { background: var(--stage-bad-bg); color: var(--stage-danger); }
</style>
