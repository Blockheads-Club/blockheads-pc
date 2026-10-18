<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { IconName } from '../icons';
  import Icon from './Icon.svelte';

  interface Props {
    tone?: 'info' | 'ok' | 'warn' | 'bad';
    title: string;
    icon?: IconName;
    children?: Snippet;
    actions?: Snippet;
  }
  let { tone = 'info', title, icon, children, actions }: Props = $props();
  const defaults = { info: 'info', ok: 'check', warn: 'alert', bad: 'alert' } as const;
</script>

<div class={['alert', tone]} role={tone === 'bad' ? 'alert' : 'note'}>
  <span class="ic"><Icon name={icon ?? defaults[tone]} /></span>
  <div class="body">
    <p class="title">{title}</p>
    {#if children}<p class="muted text">{@render children()}</p>{/if}
  </div>
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</div>

<style>
  .alert {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 14px 16px;
    border-radius: 14px;
    border: 2px solid transparent;
    flex-wrap: wrap;
  }
  .info { background: var(--info-soft); border-color: color-mix(in oklab, var(--info), transparent 70%); }
  .ok { background: var(--ok-soft); border-color: color-mix(in oklab, var(--ok), transparent 70%); }
  .warn { background: var(--warn-soft); border-color: color-mix(in oklab, var(--warn), transparent 65%); }
  .bad { background: var(--bad-soft); border-color: color-mix(in oklab, var(--bad), transparent 60%); }
  .ic { margin-top: 2px; display: flex; }
  .info .ic { color: var(--info); }
  .ok .ic { color: var(--ok); }
  .warn .ic { color: var(--warn); }
  .bad .ic { color: var(--bad); }
  .body { flex: 1 1 240px; min-width: 0; }
  .title { font-weight: 700; }
  .text { font-size: 14.5px; }
  .actions { display: flex; gap: 8px; flex-wrap: wrap; }
</style>
