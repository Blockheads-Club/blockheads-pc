<script lang="ts">
  import type { Snippet } from 'svelte';
  import Spinner from './Spinner.svelte';

  interface Props {
    size?: 'lg' | 'md';
    disabled?: boolean;
    busy?: boolean;
    describedby?: string;
    onclick?: () => void;
    children: Snippet;
  }
  let { size = 'lg', disabled = false, busy = false, describedby, onclick, children }: Props = $props();
</script>

<button class={['play', size]} {disabled} aria-busy={busy || undefined} aria-describedby={describedby} {onclick}>
  {#if busy}
    <Spinner size={22} light />
  {:else}
    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 4.5v15l12-7.5z" /></svg>
  {/if}
  {@render children()}
</button>

<style>
  .play {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    min-height: 76px;
    min-width: 280px;
    padding: 0 40px;
    border-radius: 18px;
    border: 3px solid var(--play-ink);
    background: var(--play-bg);
    color: var(--play-ink);
    font: 800 28px/1 var(--f-display);
    letter-spacing: -0.01em;
    box-shadow: 0 6px 0 var(--play-edge), 0 6px 0 3px var(--play-ink);
    cursor: pointer;
    transition: transform var(--dur-press), box-shadow var(--dur-press);
  }
  .play.md { min-height: 64px; min-width: 200px; padding: 0 30px; font-size: 24px; gap: 12px; border-radius: 16px; box-shadow: 0 5px 0 var(--play-edge), 0 5px 0 3px var(--play-ink); }
  .play:active:not(:disabled) { transform: translateY(4px); box-shadow: 0 2px 0 var(--play-edge), 0 2px 0 3px var(--play-ink); }
  .play:focus-visible { outline: 3px solid #86b9ff; outline-offset: 4px; }
  .play svg { width: 30px; height: 30px; fill: currentColor; stroke: none; flex: none; }
  .md svg { width: 26px; height: 26px; }
  .play:disabled {
    background: var(--play-disabled);
    color: var(--stage-text-2);
    border-color: #54483f;
    box-shadow: 0 4px 0 #000;
    cursor: not-allowed;
  }
</style>
