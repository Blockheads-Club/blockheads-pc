<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { IconName } from '../icons';
  import Icon from './Icon.svelte';

  interface Props {
    tone?: 'neutral' | 'ok' | 'warn' | 'bad' | 'info' | 'primary' | 'sun';
    icon?: IconName;
    dot?: boolean;
    pulse?: boolean;
    label?: string;
    children: Snippet;
  }
  let { tone = 'neutral', icon, dot = false, pulse = false, label, children }: Props = $props();
</script>

<span class={['badge', tone]} aria-label={label}>
  {#if dot}<span class="dot" class:pulse></span>{/if}
  {#if icon}<Icon name={icon} size={14} />{/if}
  {@render children()}
</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border-radius: 999px;
    font: 700 12.5px/1.4 var(--f-body);
    background: var(--surface-2);
    color: var(--text-2);
    white-space: nowrap;
  }
  .ok { background: var(--ok-soft); color: var(--ok); }
  .warn { background: var(--warn-soft); color: var(--warn); }
  .bad { background: var(--bad-soft); color: var(--bad); }
  .info { background: var(--info-soft); color: var(--info); }
  .primary { background: var(--primary-soft); color: var(--primary-ink); }
  .sun { background: var(--sun); color: #1f1a17; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: currentColor; flex: none; }
  .pulse { animation: pulse 2s ease-in-out infinite; }
  @keyframes pulse { 50% { opacity: 0.35; } }
</style>
