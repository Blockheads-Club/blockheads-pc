<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '../i18n.svelte';
  import { bytes } from '../format';
  import { store } from '../store.svelte';
  import { baseName } from '../util';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import Row from '../ui/Row.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const a = $derived(store.apk);
  const cfg = $derived(store.config);
  onMount(() => void store.refreshDataSize());
</script>

<SettingsGroup title={t('set.game')}>
  <Row id="set-game-file" first title={a?.version ? t('apk.game', { v: a.version }) : t('apk.gameUnknown')} hint={cfg ? `${a ? bytes(a.size_bytes) + ' · ' : ''}${baseName(cfg.apk)}` : ''} mono>
    {#snippet lead()}<span class="fi"><Icon name="file" /></span>{/snippet}
    {#if a?.status === 'supported'}<Badge tone="ok" icon="check">{t('apk.supported')}</Badge>
    {:else if a?.status === 'unknown'}<Badge tone="warn" icon="flask">{t('apk.unknownBadge')}</Badge>
    {:else if a}<Badge tone="bad" icon="alert">{t(`apk.err.${a.problem ?? 'damaged'}` as 'apk.err.damaged')}</Badge>{/if}
    <Button size="sm" loading={store.apkChecking} onclick={() => store.browseApk()}>{t('set.change')}</Button>
    <Button size="sm" variant="ghost" onclick={() => store.recheckApk()}>{t('set.checkAgain')}</Button>
  </Row>
  <Row id="set-game-unknown" title={t('apk.unknownTitle')} hint={t('set.unknownHint')}>
    <Toggle label={t('apk.unknownTitle')} hideLabel checked={cfg?.force ?? false} onchange={(v) => store.setForce(v)} />
  </Row>
</SettingsGroup>

<SettingsGroup title={t('set.data')}>
  <Row id="set-data-folder" first title={t('set.dataLead')} hint={`${cfg?.data_dir ?? ''}${store.dataSize !== null ? ` · ${bytes(store.dataSize)}` : ''}`} mono>
    <Button size="sm" icon="folder" onclick={() => store.openDataFolder()}>{t('set.open')}</Button>
    <Button size="sm" variant="ghost" onclick={() => store.moveData()}>{t('set.move')}</Button>
  </Row>
</SettingsGroup>

<style>
  .fi { width: 44px; height: 44px; border-radius: 12px; background: var(--surface-2); display: grid; place-items: center; flex: none; }
</style>
