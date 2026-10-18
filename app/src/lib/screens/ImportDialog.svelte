<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import ImportPanel from '../parts/ImportPanel.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
</script>

<Dialog title={t('imp.title')} description={t('imp.lead')} wide onclose={() => (store.dialog = null)}>
  <ImportPanel actions />
  {#snippet footer()}
    <Button variant="primary" onclick={() => (store.dialog = null)}>{t('common.close')}</Button>
  {/snippet}
</Dialog>
