<script lang="ts">
  import { api, win } from '../api';
  import { t } from '../i18n.svelte';
  import { REPO_URL, store } from '../store.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import Icon from '../ui/Icon.svelte';
  import Progress from '../ui/Progress.svelte';
  import Spinner from '../ui/Spinner.svelte';

  const u = $derived(store.update);
  const stage = $derived(u.stage);
  const playing = $derived(store.active && stage === 'ready');
  const p = $derived(u.progress);
  const name = $derived(u.info?.version ?? '');
  const titles = $derived({
    available: t('upd.t.available', { v: name }),
    downloading: t('upd.t.downloading', { v: name }),
    installing: t('upd.t.installing', { v: name }),
    ready: t('upd.t.ready'),
    error: t('upd.t.error')
  } as Record<string, string>);
  const bodies = $derived({
    available: t('upd.b.available', { v: store.info?.version ?? '' }),
    downloading: t('upd.b.downloading'),
    installing: t('upd.b.installing'),
    ready: playing ? t('upd.b.playing') : t('upd.b.ready'),
    error: t('upd.b.error')
  } as Record<string, string>);
  const badge = $derived(stage === 'available' ? t('upd.badge.available') : stage === 'ready' ? t('upd.badge.ready') : stage === 'error' ? t('upd.badge.error') : t('upd.badge.updating'));
  const close = () => (store.dialog = null);
</script>

<Dialog title={titles[stage] ?? titles.available} onclose={close} alert={stage === 'error'}>
  <div class="head">
    <img src="/pc-icon-macos.svg" alt="" width="64" height="64" />
    <Badge tone={stage === 'error' ? 'bad' : 'neutral'}>{badge}</Badge>
  </div>
  <p class="muted body">{bodies[stage] ?? bodies.available}</p>

  {#if stage === 'available' && u.info}
    <ul class="notes" aria-label={t('upd.notes')}>{#each u.info.notes as n (n)}<li>{n}</li>{/each}</ul>
    <p class="hint">{t('upd.size', { n: u.info.size_mb })} · {t('upd.kept')}</p>
  {:else if stage === 'downloading'}
    <div class="line"><strong>{t('upd.downloading')}</strong><span class="mono">{`${p?.percent ?? 0} %`}</span></div>
    <Progress label={t('upd.progress')} value={p?.percent ?? 0} valuetext={t('upd.progressText', { n: p?.percent ?? 0, s: p?.eta_s ?? 0 })} />
    {#if p}<p class="hint mono">{t('upd.stats', { done: p.done_mb, total: p.total_mb, speed: p.mb_per_s, eta: p.eta_s })}</p>{/if}
    <p class="hint">{t('upd.keepGoing')}</p>
  {:else if stage === 'installing'}
    <Progress label={t('upd.installing')} />
    <div class="stp"><span class="si ok"><Icon name="check" size={16} /></span>{t('upd.verified')}</div>
    <div class="stp"><span class="si"><Spinner size={16} /></span><strong>{t('upd.installingStep')}</strong></div>
    <div class="stp dim"><span class="si"><Icon name="dot" size={16} /></span>{t('upd.restart')}</div>
    <p class="hint">{t('upd.dontOff')}</p>
  {:else if stage === 'ready'}
    <div class={['note', playing ? 'warn' : 'ok']} role="status">
      <Icon name={playing ? 'alert' : 'check'} /><span>{playing ? t('upd.playing') : t('upd.installed')}</span>
    </div>
  {:else if stage === 'error'}
    <div class="note bad" role="alert"><Icon name="alert" /><span><strong>{t('upd.failed')}</strong> <span>{u.error}</span></span></div>
  {/if}

  {#snippet footer()}
    {#if stage === 'available'}
      <Button variant="ghost" onclick={close}>{t('upd.later')}</Button>
      <Button variant="primary" icon="download" onclick={() => store.installUpdate()}>{t('upd.install')}</Button>
    {:else if stage === 'downloading'}
      <Button variant="ghost" onclick={close}>{t('upd.hide')}</Button>
      <Button onclick={() => store.cancelUpdate()}>{t('upd.cancel')}</Button>
    {:else if stage === 'installing'}
      <Button disabled>{t('upd.restartNow')}</Button>
    {:else if stage === 'ready'}
      {#if playing}
        <Button variant="ghost" onclick={close}>{t('common.ok')}</Button>
        <Button disabled>{t('upd.restartNow')}</Button>
      {:else}
        <Button variant="ghost" onclick={close}>{t('upd.restartLater')}</Button>
        <Button variant="primary" icon="refresh" onclick={() => win.close()}>{t('upd.restartNow')}</Button>
      {/if}
    {:else}
      <Button variant="ghost" icon="ext" onclick={() => api.openUrl(REPO_URL)}>{t('upd.website')}</Button>
      <Button variant="primary" icon="refresh" onclick={() => store.installUpdate()}>{t('common.retry')}</Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .head { display: flex; gap: 16px; align-items: center; }
  .body { font-size: 15.5px; }
  .notes { margin: 0; padding: 12px 14px 12px 32px; border-radius: 12px; background: var(--surface-2); display: flex; flex-direction: column; gap: 6px; font-size: 14.5px; }
  .line { display: flex; justify-content: space-between; gap: 12px; font-size: 14.5px; }
  .stp { display: flex; gap: 10px; align-items: center; font-size: 14.5px; }
  .stp.dim { color: var(--text-3); }
  .si { width: 24px; height: 24px; border-radius: 7px; display: grid; place-items: center; flex: none; background: var(--surface-2); }
  .si.ok { background: var(--ok-soft); color: var(--ok); }
  .note { display: flex; gap: 10px; align-items: flex-start; padding: 12px 14px; border-radius: 12px; }
  .note.ok { background: var(--ok-soft); } .note.ok :global(.ico) { color: var(--ok); }
  .note.warn { background: var(--warn-soft); } .note.warn :global(.ico) { color: var(--warn); }
  .note.bad { background: var(--bad-soft); } .note.bad :global(.ico) { color: var(--bad); }
</style>
