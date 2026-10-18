<script lang="ts">
  import { tick } from 'svelte';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';

  let box: HTMLElement | undefined = $state();
  $effect(() => {
    void store.log.length;
    void tick().then(() => box && (box.scrollTop = box.scrollHeight));
  });
</script>

<Dialog title={t('logs.title')} description={t('logs.lead')} wide onclose={() => (store.dialog = null)}>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <pre class="log" role="log" tabindex="0" aria-label={t('logs.title')} bind:this={box}>{store.log.length ? store.log.join('\n') : t('logs.empty')}</pre>
  {#snippet footer()}
    <Button icon="copy" disabled={!store.log.length} onclick={() => store.copyLog()}>{t('logs.copy')}</Button>
    <Button icon="download" disabled={!store.log.length} onclick={() => store.saveLog()}>{t('logs.save')}</Button>
    <Button variant="primary" onclick={() => (store.dialog = null)}>{t('common.close')}</Button>
  {/snippet}
</Dialog>

<style>
  .log {
    background: var(--code-bg);
    color: var(--code-text, #f5ede5);
    font: 12.5px/1.6 var(--f-mono);
    padding: 12px 14px;
    border-radius: 12px;
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    height: min(340px, 50vh);
    overflow: auto;
  }
</style>
