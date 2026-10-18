<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import GraphicsForm from '../parts/GraphicsForm.svelte';
  import SetupShell from '../layout/SetupShell.svelte';
  import Badge from '../ui/Badge.svelte';
  import Card from '../ui/Card.svelte';

  const d = $derived(store.display);
  const scale = $derived(Math.round((store.cfg?.window.ui_scale ?? 1) * 100));
  const k = $derived(scale / 100);
</script>

<SetupShell note={t('gfx.railNote')}>
  <div class="head">
    <h1>{t('gfx.title')}</h1>
    <p class="muted lead">{t('gfx.lead')}</p>
  </div>
  {#if d}
    <Badge tone="info" icon="monitor">{t('gfx.detected', { w: d.width, h: d.height, s: Math.round(d.scale * 100) })}</Badge>
  {/if}
  <div class="split">
    <Card pad={false} label={t('gfx.options')}>
      <div class="form"><GraphicsForm /></div>
    </Card>
    <Card label={t('gfx.preview')} class="prev">
      <p class="pt">{t('gfx.preview')}</p>
      <div class="gw" role="img" aria-label={t('gfx.previewAlt', { n: scale })}>
        <span class="hud" style:left="6%" style:top="8%" style:width="{20 * k}%" style:height="{11 * k}%"></span>
        <span class="hud" style:right="6%" style:top="8%" style:width="{11 * k}%" style:height="{11 * k}%"></span>
        <span class="hud" style:left="50%" style:bottom="8%" style:transform="translateX(-50%)" style:width="{40 * k}%" style:height="{11 * k}%"></span>
      </div>
      <p class="hint">{t('gfx.previewHint')}</p>
    </Card>
  </div>
</SetupShell>

<style>
  .head { display: flex; flex-direction: column; gap: 6px; }
  h1 { font-size: 32px; }
  .lead { font-size: 16.5px; }
  .split { display: flex; gap: 16px; flex-wrap: wrap; align-items: flex-start; }
  .split > :global(:first-child) { flex: 3 1 360px; }
  .split > :global(:last-child) { flex: 1 1 240px; max-width: 300px; display: flex; flex-direction: column; gap: 10px; }
  .form { padding: 4px 20px; }
  .pt { font-weight: 700; font-size: 14.5px; }
  .gw { position: relative; aspect-ratio: 16 / 9; background: #0e0b09; border-radius: 10px; border: 2px solid var(--line-strong); overflow: hidden; }
  .hud { position: absolute; border-radius: 6px; background: rgba(255, 210, 122, 0.85); max-width: 90%; }
</style>
