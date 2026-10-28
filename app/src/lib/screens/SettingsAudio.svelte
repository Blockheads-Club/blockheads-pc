<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Button from '../ui/Button.svelte';
  import Row from '../ui/Row.svelte';
  import SelectField from '../ui/SelectField.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const master = $derived(store.cfg?.voice?.master ?? 100);
  const device = $derived(store.cfg?.voice?.mic_device ?? '');
  let dragged = $state<number | null>(null);
  const shown = $derived(dragged ?? master);
  // The chosen device stays in the list even when the runner does not see it right now (unplugged).
  const devices = $derived([...(store.mics ?? []), ...(device && !(store.mics ?? []).includes(device) ? [device] : [])]);
  const options = $derived([
    { value: '', label: t('set.micDefault') },
    ...devices.map((d) => ({ value: d, label: d }))
  ]);

  onMount(() => {
    if (store.mics === null) void store.loadMics();
  });
</script>

<SettingsGroup title={t('set.audio')}>
  <Row id="set-snd-sound" first title={t('set.sound')} hint={t('set.soundHint')}>
    <Toggle label={t('set.sound')} hideLabel checked={store.prefs.game_sound} onchange={(v) => store.patchPrefs({ game_sound: v })} />
  </Row>
  <Row id="set-snd-mute" title={t('set.muteBg')} hint={t('set.muteSoon')}>
    <Toggle label={t('set.muteBg')} hideLabel disabled checked={store.prefs.mute_in_background} />
  </Row>
</SettingsGroup>

<SettingsGroup title={t('set.voice')}>
  <Row id="set-snd-voice" first title={t('set.voiceVolume')} hint={t('set.voiceVolumeHint')}>
    <input
      type="range"
      min="0"
      max="200"
      step="10"
      value={master}
      aria-label={t('set.voiceVolume')}
      aria-valuetext={t('gfx.percent', { n: shown })}
      oninput={(e) => (dragged = Number(e.currentTarget.value))}
      onchange={(e) => {
        const v = Number(e.currentTarget.value);
        dragged = null;
        void store.setVoiceMaster(v);
      }}
    />
    <span class="mono val">{t('gfx.percent', { n: shown })}</span>
  </Row>
  <Row id="set-snd-mic" title={t('set.mic')} hint={t('set.micHint')}>
    <SelectField label={t('set.mic')} hideLabel value={device} {options} onchange={(v) => store.setMicDevice(v)} />
    <Button size="sm" icon="refresh" label={t('set.micRefresh')} onclick={() => store.loadMics()} />
  </Row>
</SettingsGroup>

<style>
  input[type='range'] { accent-color: var(--primary); width: 160px; height: 28px; }
  .val { font-weight: 600; width: 56px; text-align: right; }
</style>
