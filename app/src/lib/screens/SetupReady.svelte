<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import type { IconName } from '../icons';
  import { conflicts } from '../util';
  import { fpsPositionLabel } from '../fps-position';
  import SetupShell from '../layout/SetupShell.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';

  const d = $derived(store.draft);
  const a = $derived(store.apk);
  const changed = $derived(
    d && store.defaults
      ? (Object.keys(d.keys) as (keyof typeof d.keys)[]).filter((k) => d.keys[k] !== store.defaults?.keys[k])
      : []
  );
  const dups = $derived(d ? conflicts(d.keys) : []);
  const gfx = $derived(
    d
      ? [
          d.window.fullscreen ? t('gfx.fullscreen') : t('gfx.window'),
          `${d.window.width} × ${d.window.height}`,
          d.window.vsync ? t('ready.vsyncOn') : t('ready.vsyncOff'),
          d.window.fps_limit ? t('ready.fpsLimit', { n: d.window.fps_limit }) : t('ready.fpsUnlimited'),
          ...(d.window.show_fps ? [t('ready.showFps', { pos: fpsPositionLabel(d.window.fps_position).toLowerCase() })] : []),
          t('ready.ui', { n: Math.round(d.window.ui_scale * 100) })
        ].join(' · ')
      : ''
  );
  const imported = $derived(store.importSummary?.written ?? 0);

  const rows = $derived<{ icon: IconName; k: string; step: number; label: string }[]>([
    { icon: 'file', k: t('ready.game'), step: 2, label: t('ready.editGame') },
    { icon: 'monitor', k: t('ready.gfx'), step: 3, label: t('ready.editGfx') },
    { icon: 'mouse', k: t('ready.controls'), step: 4, label: t('ready.editControls') },
    { icon: 'map', k: t('ready.saves'), step: 5, label: t('ready.editSaves') }
  ]);
</script>

<SetupShell ready note={t('ready.railNote')}>
  {#snippet actions()}
    <Button variant="primary" size="lg" icon="rocket" onclick={() => store.finishSetup()}>{t('ready.start')}</Button>
  {/snippet}
  <div class="top">
    <img src="/pc-icon-macos.svg" alt="" width="92" height="92" />
    <div class="tt">
      <h1>{t('ready.title')}</h1>
      <p class="muted lead">{t('ready.lead')}</p>
    </div>
  </div>
  <Card pad={false} label={t('ready.summary')}>
    {#each rows as r (r.step)}
      <div class="sr">
        <span class="si"><Icon name={r.icon} /></span>
        <div class="sv">
          <p class="k">{r.k}</p>
          {#if r.step === 2}
            <p class="v">
              {a?.version ? t('apk.game', { v: a.version }) : t('apk.gameUnknown')}
              {#if a?.status === 'supported'}<Badge tone="ok" icon="check">{t('apk.supported')}</Badge>{:else}<Badge tone="warn" icon="flask">{t('apk.unknownBadge')}</Badge>{/if}
            </p>
            {#if store.prefs.keep_private_copy}<p class="hint">{t('ready.privateCopy')}</p>{/if}
          {:else if r.step === 3}
            <p class="v">{gfx}</p>
          {:else if r.step === 4}
            <p class="v">
              {changed.length ? t('ready.keysChanged', { n: changed.length }) : t('ready.keysDefault')}
              {#if changed.length}({changed.map((k) => `${t(`act.${k}` as 'act.touch')} → ${d?.keys[k]}`).join(', ')}){/if}
            </p>
            {#if dups.length}<p class="hint bad">{t('ready.keysConflict')}</p>{/if}
          {:else}
            <p class="v">{imported ? t('ready.imported', { n: imported }) : t('ready.noImport')}</p>
          {/if}
        </div>
        <Button size="sm" variant="ghost" label={r.label} onclick={() => store.jump(r.step)}>{t('common.edit')}</Button>
      </div>
    {/each}
    <div class="sr">
      <span class="si"><Icon name="folder" /></span>
      <div class="sv"><p class="k">{t('ready.dataFolder')}</p><p class="v mono">{d?.data_dir}</p></div>
    </div>
  </Card>
</SetupShell>

<style>
  .top { display: flex; gap: 18px; align-items: center; flex-wrap: wrap; }
  .tt { display: flex; flex-direction: column; gap: 6px; flex: 1; min-width: 260px; }
  h1 { font-size: 32px; }
  .lead { font-size: 16px; }
  .sr { display: flex; gap: 14px; align-items: center; padding: 14px 16px; border-top: 1px solid var(--line); }
  .sr:first-child { border-top: 0; }
  .si { width: 40px; height: 40px; border-radius: 11px; background: var(--surface-2); color: var(--primary-ink); display: grid; place-items: center; flex: none; }
  .sv { flex: 1; min-width: 0; }
  .k { font-size: 13px; font-weight: 700; color: var(--text-3); }
  .v { font-weight: 700; overflow-wrap: anywhere; }
  .v.mono { font-weight: 600; font-size: 14px; }
  .bad { color: var(--bad); }
</style>
