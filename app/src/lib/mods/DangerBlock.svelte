<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from '../i18n.svelte';
  import Icon from '../ui/Icon.svelte';

  let { title, children }: { title?: string; children?: Snippet } = $props();
</script>

<div class="dzone" role="group" aria-label={t('mods.dangerAria')}>
  <div class="dz-h"><Icon name="octagon" /><span>{title ?? t('mods.dangerTitle')}</span></div>
  {#if children}<div class="dz-b">{@render children()}</div>{/if}
</div>

<style>
  .dzone { border: 2px solid var(--mod-danger-edge); border-radius: 14px; overflow: hidden; background: var(--bad-soft); }
  .dz-h {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    color: var(--mod-on-danger);
    font: 800 14px var(--f-body);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    background: repeating-linear-gradient(-45deg, var(--mod-danger) 0 12px, var(--mod-danger-stripe) 12px 24px);
  }
  .dz-b { padding: 12px 14px; display: flex; flex-direction: column; gap: 6px; color: var(--text); font-size: 14.5px; }
  .dz-b :global(ul) { margin: 0; padding-left: 20px; }
</style>
