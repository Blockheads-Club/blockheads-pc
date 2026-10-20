<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Row from '../ui/Row.svelte';
  import Toggle from '../ui/Toggle.svelte';

  /** The game's own on-screen controls (`crates/bhpc-run/src/gameui.rs`); older configurations take the defaults. */
  const dpad = $derived(store.cfg?.show_dpad ?? false);
</script>

{#if store.cfg}
  <SettingsGroup title={t('osc.title')}>
    <Row id="set-ctl-dpad" first title={t('osc.dpad')} hint={t('osc.dpadHint')}>
      <Toggle label={t('osc.dpad')} hideLabel checked={dpad} onchange={(v) => store.setShowDpad(v)} />
    </Row>
  </SettingsGroup>
{/if}
