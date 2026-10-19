<script lang="ts">
  import { t } from '../i18n.svelte';
  import GraphicsForm from '../parts/GraphicsForm.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
</script>

<SettingsGroup title={t('set.gfx')}>
  <GraphicsForm />
  <p class="hint after">{t('set.nextStart')}</p>
</SettingsGroup>

<style>
  .after { padding: 0 0 12px; }
</style>
