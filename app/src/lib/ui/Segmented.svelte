<script lang="ts" generics="V extends string">
  import type { IconName } from '../icons';
  import Icon from './Icon.svelte';

  interface Props {
    label: string;
    value: V;
    options: { value: V; label: string; icon?: IconName }[];
    onchange: (v: V) => void;
  }
  let { label, value, options, onchange }: Props = $props();
  const uid = $props.id();
  let group: HTMLElement | undefined = $state();

  /** Radio-group keyboard model: arrows move the selection, only the checked option is in the tab order. */
  function key(e: KeyboardEvent) {
    const i = options.findIndex((o) => o.value === value);
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = options[(i + step + options.length) % options.length];
    onchange(next.value);
    group?.querySelectorAll<HTMLElement>('[role=radio]')[options.indexOf(next)]?.focus();
  }
</script>

<span class="sr" id="{uid}-l">{label}</span>
<div class="tabs" role="radiogroup" aria-labelledby="{uid}-l" bind:this={group}>
  {#each options as o (o.value)}
    <button
      class="tab"
      type="button"
      role="radio"
      aria-checked={o.value === value}
      tabindex={o.value === value ? 0 : -1}
      onclick={() => onchange(o.value)}
      onkeydown={key}
    >
      {#if o.icon}<Icon name={o.icon} />{/if}{o.label}
    </button>
  {/each}
</div>

<style>
  .tabs {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px;
    background: var(--surface-2);
    border: 2px solid var(--line);
    border-radius: 14px;
  }
  .tab {
    min-height: 40px;
    padding: 0 14px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--text-2);
    font: 700 14px/1 var(--f-body);
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }
  .tab:hover { color: var(--text); }
  .tab[aria-checked='true'] { background: var(--surface); color: var(--text); box-shadow: 0 2px 0 var(--line-strong); }
</style>
