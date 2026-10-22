<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { IconName } from '../icons';
  import Icon from '../ui/Icon.svelte';

  /** A banner of the mods design: icon, bold title, text, actions. Errors announce themselves (`role="alert"`). */
  let { tone = 'info', icon, title, role, children, actions }: {
    tone?: 'info' | 'warn' | 'bad';
    icon: IconName;
    title: string;
    role?: 'alert' | 'status' | 'note';
    children?: Snippet;
    actions?: Snippet;
  } = $props();
</script>

<div class={['notice', tone]} {role}>
  <Icon name={icon} />
  <div class="nb">
    <p class="nt">{title}</p>
    {@render children?.()}
    {#if actions}<div class="na">{@render actions()}</div>{/if}
  </div>
</div>

<style>
  .notice { display: flex; gap: 14px; align-items: flex-start; padding: 14px 16px; border-radius: 14px; border: 2px solid transparent; }
  .notice > :global(.ico) { flex: none; margin-top: 2px; }
  .info { background: var(--info-soft); border-color: color-mix(in oklab, var(--info), transparent 65%); }
  .info > :global(.ico) { color: var(--info); }
  .warn { background: var(--warn-soft); border-color: color-mix(in oklab, var(--warn), transparent 60%); }
  .warn > :global(.ico) { color: var(--warn); }
  .bad { background: var(--bad-soft); border-color: color-mix(in oklab, var(--bad), transparent 60%); }
  .bad > :global(.ico) { color: var(--bad); }
  .nb { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; font-size: 14.5px; }
  .nt { font-weight: 700; font-size: 16px; }
  .na { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin-top: 6px; }
</style>
