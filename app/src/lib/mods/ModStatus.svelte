<script lang="ts">
  import type { IconName } from '../icons';
  import { t } from '../i18n.svelte';
  import Icon from '../ui/Icon.svelte';
  import type { ModStatus } from './types';

  let { status }: { status: ModStatus } = $props();
  /** Every status has its own icon (running: a pulsing dot); "off" also has a dashed outline. */
  const look: Record<ModStatus, { cls: string; icon?: IconName }> = {
    running: { cls: 'run' },
    ready: { cls: 'ready', icon: 'clock' },
    off: { cls: 'off', icon: 'power' },
    failed: { cls: 'fail', icon: 'x-circle' },
    blocked: { cls: 'fail', icon: 'octagon' },
    budget: { cls: 'fail', icon: 'clock-x' },
    dependency: { cls: 'dep', icon: 'link' },
    permissions: { cls: 'perm', icon: 'key' }
  };
  const l = $derived(look[status]);
</script>

<span class={['st', l.cls]} data-status={status}>
  {#if l.icon}<Icon name={l.icon} size={14} />{:else}<span class="dot"></span>{/if}{t(`mods.status.${status}`)}
</span>

<style>
  .st {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px 3px 8px;
    border-radius: 999px;
    font: 700 12.5px/1.4 var(--f-body);
    /* Long translations wrap inside the badge rather than overflow narrow columns. */
    white-space: normal;
    max-width: 100%;
  }
  .run { background: var(--ok-soft); color: var(--ok); }
  .ready { background: var(--surface-2); color: var(--text-2); }
  .off { background: var(--surface-2); color: var(--text-2); border: 1.5px dashed var(--line-strong); padding: 2px 9px 2px 7px; }
  .fail { background: var(--bad-soft); color: var(--bad); }
  .dep { background: var(--info-soft); color: var(--info); }
  .perm { background: var(--warn-soft); color: var(--warn); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: currentColor; flex: none; animation: pulse 2s ease-in-out infinite; }
  @keyframes pulse { 50% { opacity: 0.35; } }
  @media (prefers-reduced-motion: reduce) { .dot { animation: none; } }
</style>
