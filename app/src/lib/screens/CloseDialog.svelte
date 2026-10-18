<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
</script>

<Dialog title={t('close.title')} description={t('close.body')} alert onclose={() => (store.dialog = null)}>
  {#snippet footer()}
    <Button onclick={() => (store.dialog = null)}>{t('close.keep')}</Button>
    <Button variant="danger" icon="stop" onclick={() => store.stopAndClose()}>{t('close.stop')}</Button>
  {/snippet}
</Dialog>
