<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import ControlsEditor from '../parts/ControlsEditor.svelte';
  import SetupShell from '../layout/SetupShell.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
</script>

<SetupShell note={t('ctl.railNote')}>
  <div class="head">
    <div>
      <h1>{t('ctl.title')}</h1>
      <p class="muted lead">{t('ctl.lead')}</p>
    </div>
    <Button size="sm" variant="ghost" icon="reset" onclick={() => store.resetKeys()}>{t('ctl.resetAll')}</Button>
  </div>
  <div class="split">
    <div class="ed"><ControlsEditor /></div>
    <aside class="mouse" aria-label={t('ctl.mouse')}>
      <svg viewBox="0 0 120 180" width="120" height="180" role="img" aria-label={t('ctl.mouseAlt')}>
        <rect x="20" y="10" width="80" height="160" rx="40" fill="var(--surface-2)" stroke="var(--line-strong)" stroke-width="3" />
        <path d="M60 10a40 40 0 0 0-40 40h40z" fill="#FFD27A" stroke="var(--line-strong)" stroke-width="3" />
        <path d="M60 10a40 40 0 0 1 40 40H60z" fill="var(--primary-soft)" stroke="var(--line-strong)" stroke-width="3" />
        <rect x="55" y="24" width="10" height="20" rx="5" fill="var(--text)" />
      </svg>
      <ul>
        <li><span class="sw" style:background="#FFD27A"></span>{t('ctl.mouseLeft')}</li>
        <li><span class="sw" style:background="var(--primary-soft)"></span>{t('ctl.mouseRight')}</li>
        <li><span class="sw" style:background="var(--text)"></span>{t('ctl.mouseWheel')}</li>
      </ul>
    </aside>
  </div>
  <p class="hint">{t('ctl.hint')}</p>
</SetupShell>

<style>
  .head { display: flex; justify-content: space-between; align-items: flex-end; gap: 12px; flex-wrap: wrap; }
  h1 { font-size: 32px; }
  .lead { font-size: 16px; margin-top: 6px; }
  .split { display: flex; gap: 16px; flex-wrap: wrap; align-items: flex-start; }
  .ed { flex: 3 1 420px; min-width: 0; }
  .mouse { flex: 0 1 180px; background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); padding: 16px; display: flex; flex-direction: column; align-items: center; gap: 12px; }
  .mouse ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; font-size: 13.5px; width: 100%; }
  .mouse li { display: flex; gap: 8px; align-items: center; }
  .sw { width: 14px; height: 14px; border-radius: 4px; border: 2px solid var(--line-strong); flex: none; }
</style>
