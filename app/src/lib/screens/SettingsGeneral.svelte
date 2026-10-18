<script lang="ts">
  import { onMount } from 'svelte';
  import { LANGUAGES, t } from '../i18n.svelte';
  import { bytes } from '../format';
  import { store } from '../store.svelte';
  import type { ThemeChoice } from '../types';
  import { baseName } from '../util';
  import GraphicsForm from '../parts/GraphicsForm.svelte';
  import SettingsShell from '../layout/SettingsShell.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';
  import Row from '../ui/Row.svelte';
  import Segmented from '../ui/Segmented.svelte';
  import SelectField from '../ui/SelectField.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const a = $derived(store.apk);
  const cfg = $derived(store.config);
  onMount(() => void store.refreshDataSize());
</script>

<SettingsShell>
  <Card pad={false} label={t('set.game')}>
    <section class="grp" aria-labelledby="g1">
      <h2 id="g1">{t('set.game')}</h2>
      <Row first title={a?.version ? t('apk.game', { v: a.version }) : t('apk.gameUnknown')} hint={cfg ? `${a ? bytes(a.size_bytes) + ' · ' : ''}${baseName(cfg.apk)}` : ''} mono>
        {#snippet lead()}<span class="fi"><Icon name="file" /></span>{/snippet}
        {#if a?.status === 'supported'}<Badge tone="ok" icon="check">{t('apk.supported')}</Badge>
        {:else if a?.status === 'unknown'}<Badge tone="warn" icon="flask">{t('apk.unknownBadge')}</Badge>
        {:else if a}<Badge tone="bad" icon="alert">{t(`apk.err.${a.problem ?? 'damaged'}` as 'apk.err.damaged')}</Badge>{/if}
        <Button size="sm" loading={store.apkChecking} onclick={() => store.browseApk()}>{t('set.change')}</Button>
        <Button size="sm" variant="ghost" onclick={() => store.recheckApk()}>{t('set.checkAgain')}</Button>
      </Row>
      <Row title={t('apk.unknownTitle')} hint={t('set.unknownHint')}>
        <Toggle label={t('apk.unknownTitle')} hideLabel checked={cfg?.force ?? false} onchange={(v) => store.setForce(v)} />
      </Row>
    </section>
  </Card>

  <Card pad={false} label={t('set.data')}>
    <section class="grp" aria-labelledby="g2">
      <h2 id="g2">{t('set.data')}</h2>
      <Row first title={t('set.dataLead')} hint={`${cfg?.data_dir ?? ''}${store.dataSize !== null ? ` · ${bytes(store.dataSize)}` : ''}`} mono>
        <Button size="sm" icon="folder" onclick={() => store.openDataFolder()}>{t('set.open')}</Button>
        <Button size="sm" variant="ghost" onclick={() => store.moveData()}>{t('set.move')}</Button>
      </Row>
    </section>
  </Card>

  <div id="sec-gfx">
    <Card pad={false} label={t('set.gfx')}>
      <section class="grp" aria-labelledby="g3">
        <h2 id="g3">{t('set.gfx')}</h2>
        <GraphicsForm />
        <p class="hint after">{t('set.nextStart')}</p>
      </section>
    </Card>
  </div>

  <div id="sec-snd">
    <Card pad={false} label={t('set.audio')}>
      <section class="grp" aria-labelledby="g4">
        <h2 id="g4">{t('set.audio')}</h2>
        <Row first title={t('set.sound')} hint={t('set.soundHint')}>
          <Toggle label={t('set.sound')} hideLabel checked={store.prefs.game_sound} onchange={(v) => store.patchPrefs({ game_sound: v })} />
        </Row>
        <Row title={t('set.muteBg')} hint={t('set.muteSoon')}>
          <Toggle label={t('set.muteBg')} hideLabel disabled checked={store.prefs.mute_in_background} />
        </Row>
      </section>
    </Card>
  </div>

  <div id="sec-lng">
    <Card pad={false} label={t('set.lang')}>
      <section class="grp" aria-labelledby="g5">
        <h2 id="g5">{t('set.lang')}</h2>
        <Row first title={t('set.language')} hint={t('set.languageHint')}>
          <SelectField
            label={t('set.language')}
            hideLabel
            value={store.prefs.language}
            options={LANGUAGES.map((l) => ({ value: l.code, label: l.label, lang: l.code }))}
            onchange={(v) => store.patchPrefs({ language: v })}
          />
        </Row>
        <Row title={t('set.theme')}>
          <Segmented
            label={t('set.theme')}
            value={store.prefs.theme}
            options={[
              { value: 'light', label: t('theme.light'), icon: 'sun' },
              { value: 'dark', label: t('theme.dark'), icon: 'moon' },
              { value: 'system', label: t('theme.system'), icon: 'monitor' }
            ]}
            onchange={(v: ThemeChoice) => store.patchPrefs({ theme: v })}
          />
        </Row>
      </section>
    </Card>
  </div>
</SettingsShell>

<style>
  .grp { padding: 4px 20px; }
  h2 { font-size: 19px; padding: 14px 0 4px; }
  .fi { width: 44px; height: 44px; border-radius: 12px; background: var(--surface-2); display: grid; place-items: center; flex: none; }
  .after { padding: 0 0 12px; }
</style>
