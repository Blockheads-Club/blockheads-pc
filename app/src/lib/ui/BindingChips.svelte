<script lang="ts">
  import { t } from '../i18n.svelte';
  import { chips } from '../util';
  import Icon from './Icon.svelte';

  let { binding, conflict = false }: { binding: string; conflict?: boolean } = $props();
  const parts = $derived(
    chips(binding, (m) => t(`input.${m.toLowerCase()}` as 'input.left'))
  );
</script>

<span class="chips">
  {#each parts as c, i (i)}
    {#if c.kind === 'key'}
      <kbd class={['key', conflict && 'bad']}>{c.text}</kbd>
    {:else if c.kind === 'mouse'}
      <span class="mb"><Icon name={c.icon} />{c.text}</span>
    {:else}
      <span class="plus" aria-hidden="true">+</span>
    {/if}
  {/each}
</span>

<style>
  .chips { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
  .key {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 34px;
    height: 32px;
    padding: 0 9px;
    border-radius: 8px;
    border: 2px solid var(--line-strong);
    background: var(--surface);
    box-shadow: 0 3px 0 var(--line-strong);
    font: 700 13.5px var(--f-body);
    color: var(--text);
  }
  .key.bad { border-color: var(--bad); box-shadow: 0 3px 0 var(--bad); }
  .mb {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 10px 0 6px;
    border-radius: 999px;
    background: var(--surface-2);
    border: 2px solid var(--line);
    font: 700 13.5px var(--f-body);
    color: var(--text);
  }
  .plus { color: var(--text-3); font-weight: 700; }
</style>
