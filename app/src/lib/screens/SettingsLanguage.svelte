<script lang="ts">
  import { LANGUAGES, t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import type { ThemeChoice } from '../types';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Row from '../ui/Row.svelte';
  import Segmented from '../ui/Segmented.svelte';
  import SelectField from '../ui/SelectField.svelte';
</script>

<SettingsGroup title={t('set.lang')}>
  <Row id="set-lng-language" first title={t('set.language')} hint={t('set.languageHint')}>
    <SelectField
      label={t('set.language')}
      hideLabel
      value={store.prefs.language}
      options={LANGUAGES.map((l) => ({ value: l.code, label: l.label, lang: l.code }))}
      onchange={(v) => store.patchPrefs({ language: v })}
    />
  </Row>
  <Row id="set-lng-theme" title={t('set.theme')}>
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
</SettingsGroup>
