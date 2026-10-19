<script lang="ts">
  import type { Snippet } from 'svelte';
  import Card from '../ui/Card.svelte';

  /** A titled card of setting rows on a Settings page. */
  let { title, id, children }: { title: string; id?: string; children: Snippet } = $props();
  const uid = $props.id();
</script>

<Card pad={false} label={title}>
  <section class="grp" aria-labelledby="{uid}-h" {id}>
    <h2 id="{uid}-h">{title}</h2>
    {@render children()}
  </section>
</Card>

<style>
  .grp { padding: 4px 20px; scroll-margin: 24px; }
  h2 { font-size: 19px; padding: 14px 0 4px; }
</style>
