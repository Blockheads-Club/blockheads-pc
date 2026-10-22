<script lang="ts">
  import { onMount } from 'svelte';
  import { api, win } from '../api';
  import { bytes } from '../format';
  import { t } from '../i18n.svelte';
  import type { IconName } from '../icons';
  import { REPO_URL, store } from '../store.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import Icon from '../ui/Icon.svelte';
  import Progress from '../ui/Progress.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import Badges from './Badges.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import DangerBlock from './DangerBlock.svelte';
  import ModIcon from './ModIcon.svelte';
  import { checkLines, errorReport, errorText, mods, sidesOf } from './mods.svelte';
  import Notice from './Notice.svelte';
  import { permInfo, permKey } from './perms';
  import SidesBadge from './SidesBadge.svelte';

  const ins = $derived(mods.install!);
  /** Kept while the dialog closes (the parent removes this component right after). */
  const plan = $derived(ins.plan);
  const first = $derived(plan?.items[0]);
  const many = $derived(plan.items.length > 1 || plan.missing.length > 0);
  const native = $derived(plan.items.filter((i) => i.native));
  const unreviewed = $derived(plan.items.some((i) => !i.reviewed));
  const blocked = $derived(store.prefs.mods_block_native);
  const authors = $derived([...new Set(native.flatMap((i) => i.authors))].join(', '));

  const title = $derived.by(() => {
    switch (ins.stage) {
      case 'source': return t('mods.inst.sourceTitle');
      case 'checking': return t('mods.inst.checking');
      case 'plan': return t('mods.inst.planTitle', { name: first!.name });
      case 'perms': return many ? t('mods.inst.permsTitleMany') : t('mods.inst.permsTitle', { name: first!.name });
      case 'native': return t('mods.inst.nativeTitle', { name: native.map((i) => i.name).join(', ') });
      case 'installing': return t('mods.inst.installing', { n: plan.items.length });
      case 'done': return t('mods.inst.done', { name: first!.name });
      case 'newer': return t('mods.newer.title');
      default: return ins.error?.code === 'checksum' || ins.error?.code === 'size' ? t('mods.inst.checkFailed') : t('mods.inst.failed');
    }
  });
  const icon = $derived<IconName>(
    ({ source: 'zip', checking: 'loader', plan: 'download', perms: 'shield', native: 'octagon', installing: 'download', done: 'check', newer: 'upd', failed: 'x-circle' } as const)[ins.stage]
  );
  const steps = $derived(
    ins.progress
      ? [
          { k: 'downloading', done: ins.progress.stage !== 'downloading', label: t('mods.inst.stDownload', { done: ins.progress.done, total: ins.progress.total }) },
          { k: 'checking', done: ins.progress.stage === 'writing', label: t('mods.inst.stCheck') },
          { k: 'writing', done: false, label: t('mods.inst.stWrite') }
        ]
      : []
  );
  const percent = $derived(ins.progress ? (ins.progress.stage === 'writing' ? 90 : ins.progress.stage === 'checking' ? 70 : Math.round((ins.progress.done / Math.max(ins.progress.total, 1)) * 60)) : undefined);

  onMount(() => {
    let off: (() => void) | undefined;
    // A .bhmod dropped on the window while the dialog asks for a file.
    void win
      .onFileDrop((paths) => {
        if (mods.install?.stage === 'source' && paths[0]) void mods.inspectFile(paths[0]);
      })
      .then((u) => (off = u))
      .catch(() => {});
    return () => off?.();
  });
</script>

<!-- The native confirm is a new dialog for focus: it starts on the safe button. -->
{#if mods.install}
{#key ins.stage === 'native'}
<Dialog {title} {icon} tone={ins.stage === 'failed' || ins.stage === 'native' ? 'bad' : 'plain'} wide={ins.stage === 'native'} onclose={() => mods.close()}>
  {#if ins.stage === 'source'}
    <div class="drop">
      <p class="dt">{t('mods.inst.drop')}</p>
      <Button size="sm" icon="folder" onclick={() => mods.pickFile()}>{t('mods.inst.choose')}</Button>
    </div>
    <div class="field">
      <label for="mods-link">{t('mods.inst.link')}</label>
      <input id="mods-link" class="input mono" type="url" placeholder="https://…/mod.bhmod" bind:value={ins.link} onkeydown={(e) => e.key === 'Enter' && ins.link.trim() && mods.inspectLink()} />
    </div>
    <Notice tone="warn" icon="alert" role="note" title={t('mods.inst.notReviewed')}><p class="muted">{t('mods.inst.notReviewedBody')}</p></Notice>
  {:else if ins.stage === 'checking'}
    <p class="busy"><Spinner />{t('mods.inst.checkingBody')}</p>
  {:else if ins.stage === 'plan'}
    <Badge tone="primary">{t('mods.inst.step1')}</Badge>
    <p class="muted">{plan.items.length > 1 ? t('mods.inst.needsMore', { n: plan.items.length - 1 }) : t('mods.inst.justThis')}</p>
    <ul class="card items">
      {#each plan.items as it (it.id)}
        <li>
          <ModIcon id={it.id} name={it.name} size="sm" />
          <div class="mm">
            <p class="mt"><b>{it.name}</b> <span class="mono hint">{it.version}</span>
              {#if it.needed_by}<Badge tone="info" icon="link">{t('mods.inst.neededBy', { name: it.needed_by })}</Badge>{/if}
              {#if it.replaces}<Badge>{t('mods.inst.replaces', { v: it.replaces })}</Badge>{/if}
            </p>
            <p class="mb"><SidesBadge sides={sidesOf(it.sides)} />{#if it.native}<DangerBadge />{/if}{#if it.reviewed}<Badges kind="reviewed" />{:else}<Badges kind="ext" />{/if}</p>
          </div>
          {#if it.size}<span class="mono hint">{bytes(it.size)}</span>{/if}
        </li>
      {/each}
    </ul>
    {#if plan.missing.length}
      <Notice tone="warn" icon="link" title={t('mods.inst.missing')}>
        <ul class="miss">{#each plan.missing as m (m.id)}<li>{t('mods.inst.missingItem', { id: m.id, req: m.req, by: m.needed_by })}</li>{/each}</ul>
      </Notice>
    {/if}
    <p class="hint">{t('mods.inst.total', { size: bytes(plan.total_bytes) })}{#if plan.satisfied.length} · {t('mods.inst.satisfied', { n: plan.satisfied.length })}{/if}</p>
  {:else if ins.stage === 'perms'}
    {#if many}<Badge tone="primary">{t('mods.inst.step2')}</Badge>{/if}
    <p class="muted">{t('mods.inst.permsLead')}</p>
    {#if unreviewed}
      <Notice tone="warn" icon="alert" role="note" title={t('mods.inst.notReviewed')}><p class="muted">{t('mods.inst.notReviewedBody')}</p></Notice>
    {/if}
    {#each plan.items as it (it.id)}
      {@const asks = it.permissions.filter((p) => p !== 'native')}
      {#if asks.length || it.native}
        <div class="card pc">
          <p class="pn">{it.name}</p>
          {#each asks as p (p)}
            {@const info = permInfo(p)}
            <label class="prow">
              <input type="checkbox" class="chk" checked={ins.grants[it.id]?.includes(p)} onchange={(e) => mods.toggleGrant(it.id, p, e.currentTarget.checked)} />
              <span class={['pi', info.tier === 1 && 'l1', info.tier === 2 && 'l2']}><Icon name={info.icon} /></span>
              <span class="pb"><span class="pt">{t(permKey(info, '.title'), { host: info.host ?? '', name: p })}</span><span class="muted pd">{t(permKey(info, '.desc'), { host: info.host ?? '', name: p })}</span></span>
            </label>
          {/each}
          {#if it.native}
            <div class="prow"><span class="pi nat"><Icon name="octagon" /></span><span class="pb"><span class="pt">{t('perm.native.title')} <DangerBadge variant="word" /></span><span class="muted pd">{t('mods.inst.nativeNext')}</span></span></div>
          {/if}
        </div>
      {:else}
        <div class="card pc row"><span class="pi"><Icon name="shield-o" /></span><p><strong>{it.name}</strong> <span class="muted">{t('mods.inst.nothing')}</span></p></div>
      {/if}
    {/each}
    <p class="hint">{t('mods.inst.sandbox')}</p>
  {:else if ins.stage === 'native'}
    <DangerBadge variant="short" />
    {#if blocked}
      <Notice tone="bad" icon="octagon" role="alert" title={t('mods.blockedTitle')}><p class="muted">{t('mods.inst.blockedBody')}</p></Notice>
    {:else}
      <DangerBlock>
        <p>{t('mods.inst.nativeIntro', { name: native.map((i) => i.name).join(', ') })}</p>
        <ul><li>{t('mods.inst.native1')}</li><li>{t('mods.inst.native2')}</li><li>{t('mods.inst.native3')}</li></ul>
        <p>{native.every((i) => i.reviewed) ? t('mods.inst.nativeReviewed') : t('mods.inst.nativeUnreviewed')}</p>
      </DangerBlock>
      <fieldset class="ticks">
        <legend class="sr">{t('mods.inst.confirm')}</legend>
        <label class="tick"><input type="checkbox" class="chk bad" bind:checked={ins.confirm[0]} />{t('mods.inst.tick1')}</label>
        <label class="tick"><input type="checkbox" class="chk bad" bind:checked={ins.confirm[1]} />{authors ? t('mods.inst.tick2', { who: authors }) : t('mods.inst.tick2Anon')}</label>
      </fieldset>
      <p class="hint">{t('mods.inst.revokeLater')}</p>
    {/if}
  {:else if ins.stage === 'installing'}
    <Progress label={t('mods.inst.progress')} value={percent} valuetext={steps.find((s) => !s.done)?.label} />
    <ul class="steps" aria-live="polite">
      {#each steps as s (s.k)}<li class={[s.done && 'ok']}>{#if s.done}<Icon name="check" size={16} />{/if}{s.label}</li>{/each}
    </ul>
  {:else if ins.stage === 'done'}
    <p class="muted">{t('mods.inst.doneBody')}</p>
  {:else if ins.stage === 'newer'}
    <p class="muted">{t('mods.newer.body', { name: ins.newer!.name, needs: ins.newer!.needs })}</p>
    {#if ins.newer!.alt}<p class="muted">{t('mods.newer.alt', { v: ins.newer!.alt })}</p>{/if}
  {:else}
    {@const err = ins.error!}
    <div role="alert">
      <p class="muted">{errorText(err)}<strong class="sp">{t('mods.inst.nothingInstalled')}</strong></p>
      {#if err.expected || err.got}
        <pre class="log">{checkLines(err)}</pre>
      {/if}
    </div>
  {/if}

  {#snippet footer()}
    {#if ins.stage === 'source'}
      <Button variant="ghost" onclick={() => mods.close()}>{t('common.cancel')}</Button>
      <Button variant="primary" disabled={!ins.link.trim()} onclick={() => mods.inspectLink()}>{t('common.next')}</Button>
    {:else if ins.stage === 'plan'}
      <Button variant="ghost" onclick={() => mods.close()}>{t('common.cancel')}</Button>
      <Button variant="primary" onclick={() => (ins.stage = 'perms')}>{t('common.next')}</Button>
    {:else if ins.stage === 'perms'}
      {#if many}<Button variant="ghost" onclick={() => (ins.stage = 'plan')}>{t('common.back')}</Button>{:else}<Button variant="ghost" onclick={() => mods.close()}>{t('common.cancel')}</Button>{/if}
      <Button onclick={() => mods.confirm(false)}>{t('mods.inst.keepOff')}</Button>
      <Button variant="primary" icon="check" onclick={() => mods.confirm(true)}>{t('mods.inst.turnOn')}</Button>
    {:else if ins.stage === 'native'}
      <Button variant="primary" autofocus onclick={() => mods.close()}>{t('mods.inst.dont')}</Button>
      {#if blocked}
        <Button icon="sliders" onclick={() => { mods.close(); store.openSettings('mods'); }}>{t('mods.openSettings')}</Button>
      {:else}
        <Button variant="danger" icon="octagon" disabled={!ins.confirm[0] || !ins.confirm[1]} onclick={() => mods.commit(true)}>{t('mods.inst.allow')}</Button>
        <p class="hint full">{t('mods.inst.tickBoth')}</p>
      {/if}
    {:else if ins.stage === 'done'}
      <Button variant="ghost" onclick={() => mods.showInstalled()}>{t('mods.inst.openMine')}</Button>
      <Button variant="primary" onclick={() => mods.close()}>{t('common.close')}</Button>
    {:else if ins.stage === 'newer'}
      <Button variant="ghost" onclick={() => mods.close()}>{t('common.cancel')}</Button>
      {#if ins.newer!.alt}<Button onclick={() => mods.installFromIndex(ins.newer!.id, ins.newer!.alt)}>{t('mods.newer.installAlt', { v: ins.newer!.alt })}</Button>{/if}
      <Button variant="primary" icon="upd" onclick={() => api.openUrl(`${REPO_URL}/releases`)}>{t('mods.newer.update')}</Button>
    {:else if ins.stage === 'failed'}
      <Button variant="ghost" icon="copy" onclick={() => mods.copy(errorReport(ins.error!))}>{t('mods.inst.copy')}</Button>
      {#if ins.retry}<Button variant="primary" icon="refresh" onclick={() => ins.retry!()}>{t('mods.inst.again')}</Button>{:else}<Button variant="primary" onclick={() => mods.close()}>{t('common.close')}</Button>{/if}
    {/if}
  {/snippet}
</Dialog>
{/key}
{/if}

<style>
  .drop { border: 3px dashed var(--line-strong); border-radius: 16px; padding: 20px; display: flex; flex-direction: column; align-items: center; gap: 10px; text-align: center; }
  .dt { font-weight: 700; }
  .field { display: flex; flex-direction: column; gap: 6px; }
  .field label { font-weight: 700; font-size: 14.5px; }
  .input { min-height: 44px; padding: 0 14px; border: 2px solid var(--line-strong); border-radius: 10px; background: var(--surface); color: var(--text); font-size: 13.5px; width: 100%; }
  .busy { display: flex; gap: 12px; align-items: center; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: 16px; }
  .items { list-style: none; margin: 0; padding: 0; }
  .items li { display: flex; align-items: center; gap: 14px; padding: 12px 16px; border-top: 1px solid var(--line); }
  .items li:first-child { border-top: 0; }
  .mm { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .mt, .mb { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
  .miss { margin: 0; padding-left: 18px; font-size: 14px; }
  .pc { padding: 4px 16px; }
  .pc.row { padding: 10px 16px; display: flex; gap: 12px; align-items: center; }
  .pn { font-weight: 700; padding-top: 10px; }
  .prow { display: flex; gap: 14px; align-items: flex-start; padding: 12px 0; border-top: 1px solid var(--line); cursor: pointer; }
  .pn + .prow { border-top: 0; }
  .pi { width: 36px; height: 36px; border-radius: 10px; display: grid; place-items: center; flex: none; background: var(--surface-2); color: var(--text-2); }
  .pi.l1 { background: var(--warn-soft); color: var(--warn); }
  .pi.l2 { background: var(--bad-soft); color: var(--bad); }
  .pi.nat { background: repeating-linear-gradient(-45deg, var(--mod-danger) 0 6px, var(--mod-danger-stripe) 6px 12px); color: var(--mod-on-danger); }
  .pb { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .pt { font-weight: 700; }
  .pd { font-size: 14px; }
  .chk { width: 22px; height: 22px; accent-color: var(--primary); margin: 7px 0 0; flex: none; }
  .chk.bad { accent-color: var(--bad); margin-top: 1px; }
  .ticks { border: 0; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .tick { display: flex; gap: 12px; align-items: flex-start; padding: 12px 14px; border-radius: 12px; background: var(--surface-2); font-weight: 700; cursor: pointer; }
  .steps { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; font-size: 14.5px; }
  .steps li { display: flex; gap: 6px; align-items: center; color: var(--text-3); }
  .steps li.ok { color: var(--text); }
  .log { background: var(--code-bg); color: var(--code-text); font: 12.5px/1.7 var(--f-mono); padding: 10px 12px; border-radius: 12px; white-space: pre-wrap; margin: 8px 0 0; overflow-wrap: anywhere; }
  .full { flex-basis: 100%; text-align: right; }
  .sp { margin-left: 0.3em; }
</style>
