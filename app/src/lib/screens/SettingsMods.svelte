<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../api';
  import { bytes } from '../format';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Notice from '../mods/Notice.svelte';
  import { mods } from '../mods/mods.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Row from '../ui/Row.svelte';
  import SelectField from '../ui/SelectField.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const limits = $derived([
    { value: '500', label: '500 MB' },
    { value: '1024', label: '1 GB' },
    { value: '2048', label: '2 GB' },
    { value: '0', label: t('mods.set.noLimit') }
  ]);
  const cache = $derived(mods.servers);
  const dir = $derived(mods.overview?.dir ?? '');

  onMount(() => {
    void mods.load();
    void mods.loadServers();
  });
</script>

<Notice icon="puzzle" title={t('mods.set.manage')}>
  <p class="hint">{t('mods.set.manageBody')}</p>
  {#snippet actions()}<Button size="sm" icon="chev-r" onclick={() => mods.open('mine')}>{t('mods.set.openMods')}</Button>{/snippet}
</Notice>

<SettingsGroup title={t('mods.set.updates')}>
  <Row id="set-mods-auto" first title={t('mods.set.auto')} hint={t('mods.set.autoHint')}>
    <Toggle label={t('mods.set.auto')} hideLabel checked={store.prefs.mods_auto_update} onchange={(v) => store.patchPrefs({ mods_auto_update: v })} />
  </Row>
</SettingsGroup>

<SettingsGroup title={t('mods.set.servers')}>
  <Row id="set-mods-servers" first title={t('mods.set.fromServers')} hint={t('mods.set.fromServersHint')}>
    <Badge tone="ok" icon="check">{t('mods.set.alwaysAsks')}</Badge>
  </Row>
  <Row id="set-mods-cache" title={t('mods.set.storage')} hint={t('mods.set.storageHint')}>
    <SelectField label={t('mods.set.storage')} hideLabel value={String(store.prefs.mods_cache_limit_mb)} options={limits}
      onchange={async (v) => { await store.patchPrefs({ mods_cache_limit_mb: Number(v) }); await mods.loadServers(); }} />
  </Row>
  <Row id="set-mods-clear" title={t('mods.set.clear')} hint={cache ? t('mods.set.clearHint', { size: bytes(cache.total), n: cache.servers.length }) : ''}>
    <Button size="sm" onclick={() => mods.open('servers')}>{t('mods.set.manageCache')}</Button>
  </Row>
</SettingsGroup>

<SettingsGroup title={t('mods.set.safety')}>
  <Row id="set-mods-native" first title={t('mods.set.blockNative')} hint={t('mods.set.blockNativeHint')}>
    <Toggle label={t('mods.set.blockNative')} hideLabel checked={store.prefs.mods_block_native} onchange={async (v) => { await store.patchPrefs({ mods_block_native: v }); await mods.load(true); }} />
  </Row>
  <Row id="set-mods-files" title={t('mods.set.files')} hint={t('mods.set.filesHint')}>
    <Badge tone="ok" icon="check">{t('mods.set.alwaysOn')}</Badge>
  </Row>
</SettingsGroup>

<SettingsGroup title={t('mods.set.developer')}>
  <Row id="set-mods-dev" first title={t('mods.set.devMode')} hint={t('mods.set.devModeHint')}>
    <Toggle label={t('mods.set.devMode')} hideLabel checked={store.prefs.mods_developer} onchange={(v) => store.patchPrefs({ mods_developer: v })} />
  </Row>
  <Row id="set-mods-folder" title={t('mods.set.folder')} hint={dir} mono>
    <Button size="sm" icon="folder" disabled={!dir} onclick={() => api.openPath(dir)}>{t('mods.set.open')}</Button>
  </Row>
</SettingsGroup>
