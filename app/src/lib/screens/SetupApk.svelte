<script lang="ts">
  import { onMount } from 'svelte';
  import { api, win } from '../api';
  import { bytes } from '../format';
  import { t } from '../i18n.svelte';
  import { GUIDE_URL, store } from '../store.svelte';
  import type { IconName } from '../icons';
  import type { ApkProblem } from '../types';
  import { baseName } from '../util';
  import SetupShell from '../layout/SetupShell.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';
  import Progress from '../ui/Progress.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const a = $derived(store.apk);
  const ok = $derived(!!a && a.status !== 'error');
  const bad = $derived(!!a && a.status === 'error');
  const ICON: Record<ApkProblem, IconName> = { not_blockheads: 'x', unsupported: 'alert', damaged: 'broken', unreadable: 'alert' };
  const problem = $derived(a?.problem ?? 'damaged');
  let drag = $state(false);

  onMount(() => {
    let off: (() => void) | undefined;
    void win.onFileDrop((paths) => paths[0] && store.pickApk(paths[0])).then((u) => (off = u)).catch(() => {});
    return () => off?.();
  });
</script>

<SetupShell note={t('apk.railNote')}>
  <div class="head">
    <h1>{t('apk.title')}</h1>
    <p class="muted lead">{t('apk.lead')}</p>
  </div>

  {#if store.apkChecking}
    <Card label={t('apk.checkingLabel')}>
      <div class="res" aria-busy="true">
        <span class="blob plain"><Icon name="file" /></span>
        <div class="rb">
          <p class="fn">{baseName(a?.path ?? '')}</p>
          <p class="muted chk" role="status"><Spinner />{t('apk.checking')}</p>
          <Progress label={t('apk.checkingLabel')} />
        </div>
      </div>
    </Card>
  {:else if ok && a}
    <Card tone={a.status === 'supported' ? 'ok' : undefined} label={t('apk.resultLabel')}>
      <div class="res" role="status">
        <span class={['blob', a.status === 'supported' ? 'ok' : 'warn']}><Icon name={a.status === 'supported' ? 'check' : 'flask'} /></span>
        <div class="rb">
          <div class="tt">
            <p class="h">{a.version ? t('apk.game', { v: a.version }) : t('apk.gameUnknown')}</p>
            {#if a.status === 'supported'}<Badge tone="ok" icon="check">{t('apk.supported')}</Badge>{:else}<Badge tone="warn" icon="flask">{t('apk.unknownBadge')}</Badge>{/if}
          </div>
          <dl class="kv">
            <dt>{t('apk.file')}</dt><dd class="mono">{a.path}</dd>
            <dt>{t('apk.size')}</dt><dd>{bytes(a.size_bytes)}</dd>
          </dl>
        </div>
        <Button size="sm" onclick={() => store.browseApk()}>{t('common.change')}</Button>
      </div>
    </Card>
    <div class="keep">
      <Toggle label={t('apk.keepCopy')} checked={store.prefs.keep_private_copy} onchange={(v) => store.patchPrefs({ keep_private_copy: v })} />
    </div>
    <p class="hint keep-h">{t('apk.keepCopyHint')}</p>
  {:else if bad && a}
    <Card tone="bad" label={t('apk.resultLabel')}>
      <div class="res" role="alert">
        <span class="blob badb"><Icon name={ICON[problem]} /></span>
        <div class="rb">
          <p class="h">{t(`apk.err.${problem}` as 'apk.err.damaged')}</p>
          <p class="muted">{t(`apk.err.${problem}.body` as 'apk.err.damaged.body')}</p>
          <p class="mono hint small">{baseName(a.path) + (a.detail ? ` · ${a.detail}` : '')}</p>
          <div class="acts">
            <Button size="sm" variant="primary" icon="folder" onclick={() => store.browseApk()}>{t('apk.chooseOther')}</Button>
            <Button size="sm" variant="ghost" icon="book" onclick={() => api.openUrl(GUIDE_URL)}>{t(`apk.help.${problem}` as 'apk.help.damaged')}</Button>
          </div>
        </div>
      </div>
    </Card>
  {:else}
    <div class={['drop', drag && 'over']} aria-label={t('apk.dropLabel')} role="group"
      ondragenter={() => (drag = true)} ondragleave={() => (drag = false)} ondrop={() => (drag = false)} ondragover={(e) => e.preventDefault()}>
      <span class="blob sun"><Icon name="upload" /></span>
      <p class="dt">{t('apk.drop')}</p>
      <Button variant="primary" icon="folder" onclick={() => store.browseApk()}>{t('common.browse')}</Button>
      <p class="hint">
        {t('apk.noApk')}
        <a href={GUIDE_URL} onclick={(e) => { e.preventDefault(); void api.openUrl(GUIDE_URL); }}>{t('apk.noApkLink')}</a>
      </p>
    </div>
  {/if}

  <details class="adv card" open={store.cfg?.force || (bad && a?.problem === 'unsupported')}>
    <summary><Icon name="chev-d" />{t('apk.advanced')}</summary>
    <div class="advrow">
      <div class="advtxt">
        <p class="advt" id="unk">{t('apk.unknownTitle')}</p>
        <p class="hint">{t('apk.unknownHint')}</p>
      </div>
      <Toggle label={t('apk.unknownTitle')} hideLabel checked={store.cfg?.force ?? false} onchange={(v) => store.setForce(v)} />
    </div>
    {#if store.cfg?.force}
      <div class="exp"><Icon name="flask" /><span><strong>{t('apk.experimental')}</strong> {t('apk.experimentalBody')}</span></div>
    {/if}
  </details>
</SetupShell>

<style>
  .head { display: flex; flex-direction: column; gap: 6px; }
  h1 { font-size: 32px; }
  .lead { font-size: 16.5px; }
  .drop {
    border: 3px dashed var(--line-strong);
    border-radius: 20px;
    background: var(--surface);
    min-height: 220px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 24px;
    text-align: center;
  }
  .drop.over { border-color: var(--primary); background: var(--primary-soft); }
  .dt { font: 750 21px var(--f-display); }
  .blob { width: 68px; height: 68px; border-radius: 18px; display: grid; place-items: center; transform: rotate(-6deg); box-shadow: 0 4px 0 rgba(0, 0, 0, 0.18); flex: none; }
  .blob.sun { background: var(--sun); color: #1f1a17; }
  .blob.ok { background: var(--ok); color: #fff; }
  .blob.warn { background: var(--warn-soft); color: var(--warn); }
  .blob.badb { background: var(--bad-soft); color: var(--bad); }
  .blob.plain { background: var(--surface-2); color: var(--text-2); transform: none; box-shadow: none; }
  .res { padding: 18px 20px; display: flex; gap: 16px; align-items: flex-start; }
  .rb { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 8px; }
  .fn { font-weight: 700; overflow-wrap: anywhere; }
  .chk { display: flex; gap: 10px; align-items: center; }
  .tt { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .h { font: 750 21px var(--f-display); }
  .kv { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 4px 14px; font-size: 14.5px; margin: 0; }
  .kv dt { color: var(--text-3); font-weight: 700; }
  .kv dd { margin: 0; overflow-wrap: anywhere; }
  .small { font-size: 12.5px; overflow-wrap: anywhere; }
  .acts { display: flex; gap: 10px; flex-wrap: wrap; margin-top: 6px; }
  .keep { padding: 0 2px; }
  .keep-h { margin: -10px 0 0 60px; }
  .adv { padding: 4px 18px 14px; background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); }
  .adv summary { cursor: pointer; list-style: none; font-weight: 700; min-height: 44px; display: flex; align-items: center; gap: 8px; }
  .adv summary::-webkit-details-marker { display: none; }
  .adv[open] summary :global(.ico) { transform: rotate(180deg); }
  .advrow { display: flex; justify-content: space-between; align-items: center; gap: 16px; flex-wrap: wrap; }
  .advtxt { flex: 1 1 280px; }
  .advt { font-weight: 700; }
  .exp { display: flex; gap: 10px; align-items: flex-start; padding: 10px 12px; border-radius: 10px; background: var(--warn-soft); margin-top: 10px; font-size: 14.5px; }
  .exp :global(.ico) { color: var(--warn); margin-top: 1px; }
</style>
