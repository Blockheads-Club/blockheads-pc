<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    children,
    pad = true,
    tone,
    label,
    class: cls = ''
  }: { children: Snippet; pad?: boolean; tone?: 'primary' | 'ok' | 'bad'; label?: string; class?: string } = $props();
</script>

<section class={["card", tone, cls, pad && "pad"]} aria-label={label}>
  {@render children()}
</section>

<style>
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); min-width: 0; }
  .pad { padding: 20px; }
  .primary { border-color: var(--primary); }
  .ok { border-color: var(--ok); }
  .bad { border-color: var(--bad); }
</style>
