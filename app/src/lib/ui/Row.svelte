<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    hint?: string;
    mono?: boolean;
    titleId?: string;
    first?: boolean;
    children?: Snippet;
    lead?: Snippet;
  }
  let { title, hint, mono = false, titleId, first = false, children, lead }: Props = $props();
</script>

<div class={['row', first && 'first']}>
  <div class="row-l">
    {#if lead}{@render lead()}{/if}
    <div class="txt">
      {#if title}<p class="t" id={titleId}>{title}</p>{/if}
      {#if hint}<p class="hint" class:mono>{hint}</p>{/if}
    </div>
  </div>
  {#if children}<div class="ctl">{@render children()}</div>{/if}
</div>

<style>
  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 14px;
    padding: 13px 0;
    border-top: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .row.first { border-top: 0; }
  .row-l { flex: 1 1 220px; min-width: 0; display: flex; gap: 14px; align-items: center; }
  .txt { min-width: 0; }
  .t { font-weight: 700; }
  .hint.mono { font-size: 12.5px; overflow-wrap: anywhere; }
  .ctl { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; max-width: 100%; }
</style>
