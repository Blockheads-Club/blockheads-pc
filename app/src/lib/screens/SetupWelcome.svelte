<script lang="ts">
  import { t } from '../i18n.svelte';
  import { LANGUAGES } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import { effectiveDark } from '../theme.svelte';
  import SetupShell from '../layout/SetupShell.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import SelectField from '../ui/SelectField.svelte';

  const dark = $derived(effectiveDark());
  const points = [
    { icon: 'file' as const, bg: 'var(--sun)', fg: '#1f1a17', title: 'welcome.p1', body: 'welcome.p1b' },
    { icon: 'shield' as const, bg: 'var(--info-soft)', fg: 'var(--info)', title: 'welcome.p2', body: 'welcome.p2b' },
    { icon: 'users' as const, bg: 'var(--primary-soft)', fg: 'var(--primary-ink)', title: 'welcome.p3', body: 'welcome.p3b' }
  ] as const;
</script>

<SetupShell note={t('welcome.railNote')}>
  <div class="top">
    <img src="/pc-icon-macos.svg" alt="" width="96" height="96" />
    <div class="tools">
      <Button size="sm" pressed={dark} icon={dark ? 'sun' : 'moon'} onclick={() => store.patchPrefs({ theme: dark ? 'light' : 'dark' })}>
        {dark ? t('theme.lightMode') : t('theme.darkMode')}
      </Button>
      <span class="lang">
        <Icon name="globe" />
        <SelectField
          label={t('lang.label')}
          hideLabel
          value={store.prefs.language}
          options={LANGUAGES.map((l) => ({ value: l.code, label: l.label, lang: l.code }))}
          onchange={(v) => store.patchPrefs({ language: v })}
        />
      </span>
    </div>
  </div>
  <div class="head">
    <h1>{t('welcome.title')}</h1>
    <p class="muted lead">{t('welcome.lead')}</p>
  </div>
  <section class="card pts" aria-labelledby="bf">
    <h2 id="bf" class="sr">{t('welcome.before')}</h2>
    {#each points as p (p.title)}
      <div class="pt">
        <span class="pi" style:background={p.bg} style:color={p.fg}><Icon name={p.icon} /></span>
        <div><p class="pb">{t(p.title)}</p><p class="muted pd">{t(p.body)}</p></div>
      </div>
    {/each}
  </section>
  <label class="own">
    <input type="checkbox" class="chk" bind:checked={store.legalOk} />
    {t('welcome.own')}
  </label>
  <details class="legal">
    <summary>{t('welcome.legalLink')}</summary>
    <p class="hint selectable">{t('legal.full')}</p>
  </details>
</SetupShell>

<style>
  .top { display: flex; justify-content: space-between; align-items: flex-start; gap: 16px; flex-wrap: wrap; }
  .top img { margin: -8px 0 -8px -8px; }
  .tools { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .lang { position: relative; display: inline-flex; align-items: center; }
  .lang > :global(.ico) { position: absolute; left: 12px; z-index: 1; pointer-events: none; }
  .lang :global(select) { padding-left: 40px; min-width: 150px; }
  .head { display: flex; flex-direction: column; gap: 8px; }
  h1 { font-size: 36px; }
  .lead { font-size: 17px; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); padding: 4px 20px; }
  .pt { display: flex; gap: 14px; align-items: flex-start; padding: 14px 0; border-bottom: 1px solid var(--line); }
  .pt:last-child { border-bottom: 0; }
  .pi { width: 40px; height: 40px; border-radius: 11px; display: grid; place-items: center; flex: none; }
  .pb { font-weight: 700; }
  .pd { font-size: 15px; }
  .own { display: flex; gap: 12px; align-items: flex-start; padding: 14px 16px; border-radius: 12px; background: var(--surface-2); font-weight: 700; font-size: 15.5px; cursor: pointer; }
  .legal { font-size: 14px; }
  .legal summary { cursor: pointer; font-weight: 700; color: var(--primary-ink); min-height: 32px; }
  .legal p { margin-top: 6px; }
  .chk { width: 22px; height: 22px; accent-color: var(--primary); margin: 1px 0 0; flex: none; }
</style>
