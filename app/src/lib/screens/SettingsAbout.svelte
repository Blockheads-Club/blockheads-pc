<script lang="ts">
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { REPO_URL, store } from '../store.svelte';
  import { toast } from '../ui/toast.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import Icon from '../ui/Icon.svelte';
  import Row from '../ui/Row.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const u = $derived(store.update);
  const i = $derived(store.info ?? { version: '', os: '', arch: '', updates_enabled: false });
  const enabled = $derived(i.updates_enabled);
  let licences = $state<string | null>(null);

  async function showLicences() {
    try {
      licences = await (await fetch('/third-party-notices.txt')).text();
    } catch {
      licences = t('about.noNotices');
    }
  }
  async function copyInfo() {
    toast((await store.systemInfo()) ? t('about.copied') : t('common.copyFailed'));
  }
</script>

<SettingsGroup title={t('upd.section')}>
  <div aria-live="polite">
    {#if !enabled}
      <Row id="set-upd-check" first title={t('upd.off')} hint={t('upd.offHint')} />
    {:else if u.stage === 'checking'}
      <Row id="set-upd-check" first title={t('upd.checking')}>
        {#snippet lead()}<Spinner />{/snippet}
        <Button size="sm" disabled>{t('upd.check')}</Button>
      </Row>
    {:else if u.stage === 'uptodate'}
      <Row id="set-upd-check" first title={t('upd.latest')} hint={t('upd.checkedNow')}>
        <Badge tone="ok" icon="check">{t('upd.upToDate')}</Badge>
        <Button size="sm" variant="ghost" icon="refresh" onclick={() => store.checkUpdate()}>{t('upd.again')}</Button>
      </Row>
    {:else if u.stage === 'available' || u.stage === 'downloading' || u.stage === 'installing' || u.stage === 'ready'}
      <Row id="set-upd-check" first title={t('upd.avail', { v: u.info!.version })} hint={t('upd.availHint')}>
        <Badge tone="primary">{t('upd.new')}</Badge>
        <Button size="sm" variant="primary" icon="download" haspopup onclick={() => (store.dialog = 'update')}>{t('upd.open')}</Button>
      </Row>
    {:else if u.stage === 'error'}
      <Row id="set-upd-check" first title={t('upd.cantCheck')} hint={t('upd.cantCheckHint')}>
        <Button size="sm" icon="refresh" onclick={() => store.checkUpdate()}>{t('common.retry')}</Button>
      </Row>
    {:else}
      <Row id="set-upd-check" first title={t('upd.notChecked')} hint={`Blockheads PC ${i.version}`}>
        <Button size="sm" icon="refresh" onclick={() => store.checkUpdate()}>{t('upd.check')}</Button>
      </Row>
    {/if}
  </div>
  <Row id="set-upd-auto" title={t('upd.auto')} hint={t('upd.autoHint')}>
    <Toggle label={t('upd.auto')} hideLabel disabled={!enabled} checked={store.prefs.auto_update_check} onchange={(v) => store.patchPrefs({ auto_update_check: v })} />
  </Row>
</SettingsGroup>

<SettingsGroup title={t('about.title')}>
  <Row id="set-about-app" first title="Blockheads PC" hint={t('about.line', { v: i.version, os: i.os, arch: i.arch })}>
    {#snippet lead()}<img src="/pc-icon-macos.svg" alt="" width="64" height="64" />{/snippet}
    <Button size="sm" icon="copy" onclick={copyInfo}>{t('about.copy')}</Button>
    <Button size="sm" variant="ghost" icon="code" onclick={() => api.openUrl(REPO_URL)}>GitHub</Button>
  </Row>
</SettingsGroup>

<SettingsGroup title={t('legal.title')}>
  <Row id="set-legal" first title="">
    <p class="muted legal">{t('legal.short')}</p>
  </Row>
  <Row id="set-legal-full" title={t('legal.fullTitle')}>
    <p class="hint legal full">{t('legal.full')}</p>
  </Row>
</SettingsGroup>

<SettingsGroup title={t('lic.title')} id="set-licences">
  <div class="lic first"><span>Blockheads PC</span><span class="hint">GPL-3.0-only</span></div>
  <div class="lic"><span>{t('lic.rust')}</span><span class="hint">{t('lic.rustLic')}</span></div>
  <div class="lic"><span>Bricolage Grotesque · Atkinson Hyperlegible Next · Onest · JetBrains Mono</span><span class="hint">OFL-1.1</span></div>
  <div class="lic"><span>Svelte · SvelteKit · Tauri</span><span class="hint">MIT / Apache-2.0</span></div>
  <button class="more" onclick={showLicences}>{t('lic.all')}</button>
</SettingsGroup>

{#if licences !== null}
  <Dialog title={t('lic.title')} description={t('lic.lead')} wide onclose={() => (licences = null)}>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre class="notices" tabindex="0" aria-label={t('lic.title')}>{licences}</pre>
    {#snippet footer()}<Button variant="primary" onclick={() => (licences = null)}>{t('common.close')}</Button>{/snippet}
  </Dialog>
{/if}

<style>
  .legal { font-size: 15px; }
  .full { font-size: 13.5px; }
  .lic { display: flex; justify-content: space-between; gap: 12px; padding: 9px 0; border-top: 1px solid var(--line); font-size: 14.5px; }
  .lic.first { border-top: 0; }
  .more { font-weight: 700; font-size: 14.5px; min-height: 44px; display: inline-flex; align-items: center; background: none; border: 0; padding: 0; color: var(--primary-ink); text-decoration: underline; cursor: pointer; font-family: var(--f-body); }
  .notices { background: var(--code-bg); color: #f5ede5; font: 12px/1.55 var(--f-mono); padding: 12px 14px; border-radius: 12px; margin: 0; white-space: pre-wrap; height: min(360px, 55vh); overflow: auto; }
</style>
