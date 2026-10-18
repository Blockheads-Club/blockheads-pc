<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import ImportPanel from '../parts/ImportPanel.svelte';
  import SetupShell from '../layout/SetupShell.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';

  const choosing = $derived(!store.importing && !store.importSummary);
</script>

<SetupShell note={t('imp.railNote')}>
  {#snippet actions()}
    <div class="acts">
      {#if choosing}
        <Button variant="ghost" onclick={() => store.next()}>{t('common.skip')}</Button>
        <Button variant="primary" size="lg" disabled={!store.importPath} onclick={() => store.runImport()}>{t('imp.import')}</Button>
      {:else if store.importing}
        <Button disabled>{t('imp.working')}</Button>
      {:else}
        <Button variant="primary" size="lg" onclick={() => store.next()}>{t('common.next')}<Icon name="chev-r" /></Button>
      {/if}
    </div>
  {/snippet}
  <div class="head">
    <div class="tt"><h1>{t('imp.title')}</h1><Badge>{t('common.optional')}</Badge></div>
    <p class="muted lead">{t('imp.lead')}</p>
  </div>
  <ImportPanel />
</SetupShell>

<style>
  .head { display: flex; flex-direction: column; gap: 6px; }
  .tt { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  h1 { font-size: 32px; }
  .lead { font-size: 16px; }
  .acts { display: flex; gap: 10px; flex-wrap: wrap; }
</style>
