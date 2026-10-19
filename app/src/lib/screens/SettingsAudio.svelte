<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Row from '../ui/Row.svelte';
  import Toggle from '../ui/Toggle.svelte';
</script>

<SettingsGroup title={t('set.audio')}>
  <Row id="set-snd-sound" first title={t('set.sound')} hint={t('set.soundHint')}>
    <Toggle label={t('set.sound')} hideLabel checked={store.prefs.game_sound} onchange={(v) => store.patchPrefs({ game_sound: v })} />
  </Row>
  <Row id="set-snd-mute" title={t('set.muteBg')} hint={t('set.muteSoon')}>
    <Toggle label={t('set.muteBg')} hideLabel disabled checked={store.prefs.mute_in_background} />
  </Row>
</SettingsGroup>
