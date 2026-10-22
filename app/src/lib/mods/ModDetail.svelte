<script lang="ts">
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import Icon from '../ui/Icon.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import Badges from './Badges.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import DangerBlock from './DangerBlock.svelte';
  import ModIcon from './ModIcon.svelte';
  import { modName, modText, mods, shortDate, sidesOf } from './mods.svelte';
  import ModStatus from './ModStatus.svelte';
  import Notice from './Notice.svelte';
  import { permInfo, permKey } from './perms';
  import SidesBadge from './SidesBadge.svelte';
  import type { ModSummary } from './types';

  let { m }: { m: ModSummary } = $props();
  let confirmRemove = $state(false);
  const name = $derived(modName(m));
  const busy = $derived(mods.busy === m.id);
  const missingDep = $derived(m.dependencies.find((d) => !d.ok && !d.soft));
  const source = $derived(
    m.source === 'index' ? t('mods.from.index') : m.source === 'link' ? t('mods.from.link') : m.source === 'file' ? t('mods.from.file') : ''
  );
  const meta = $derived(
    [m.authors.length ? t('mods.by', { who: m.authors.join(', ') }) : '', source, m.installed_at ? shortDate(m.installed_at) : ''].filter(Boolean).join(' · ')
  );

  async function remove() {
    confirmRemove = false;
    await mods.remove(m);
  }
</script>

<div class="head">
  <ModIcon id={m.id} {name} size="lg" />
  <div class="ht">
    <h2>{name}<span class="mono ver">{m.version}</span></h2>
    <div class="bd">
      <SidesBadge sides={sidesOf(m.sides)} />
      <ModStatus status={m.status} />
      {#if m.native}<DangerBadge />{/if}
      {#if m.reviewed}<Badges kind="reviewed" />{:else}<Badges kind="ext" />{/if}
    </div>
    {#if meta}<p class="hint">{meta}</p>{/if}
  </div>
  <Toggle label={m.enabled ? t('mods.on') : t('mods.off')} checked={m.enabled} disabled={busy} onchange={(v) => mods.toggle(m.id, v)} />
</div>

<div class="acts">
  {#if m.update}<Button size="sm" variant="primary" icon="download" onclick={() => mods.update(m)}>{t('mods.update')}</Button>{/if}
  <Button size="sm" icon="folder" onclick={() => mods.openFolder()}>{t('mods.openFolder')}</Button>
  <Button size="sm" variant="danger-quiet" icon="trash" onclick={() => (confirmRemove = true)}>{t('mods.remove')}</Button>
</div>

{#if m.status === 'failed'}
  <Notice tone="bad" icon="x-circle" role="alert" title={t('mods.failedTitle')}>
    <p class="muted">{m.reason === 'server-only' ? t('mods.failed.serverOnly') : t('mods.failed.loader', { req: m.detail ?? '' })}</p>
  </Notice>
{:else if m.status === 'blocked'}
  <Notice tone="bad" icon="octagon" role="alert" title={t('mods.blockedTitle')}>
    <p class="muted">{t('mods.blockedBody')}</p>
    {#snippet actions()}<Button size="sm" icon="sliders" onclick={() => store.openSettings('mods')}>{t('mods.openSettings')}</Button>{/snippet}
  </Notice>
{:else if m.status === 'dependency' && missingDep}
  <Notice icon="link" role="status" title={t('mods.waitingFor', { name: missingDep.id })}>
    <p class="muted">{t('mods.waitingBody', { name, dep: missingDep.id, req: missingDep.req })}</p>
    {#snippet actions()}
      {#if missingDep.available}
        <Button size="sm" variant="primary" icon="download" onclick={() => mods.installFromIndex(missingDep.id, missingDep.available)}>
          {t('mods.installDep', { name: missingDep.id, v: missingDep.available })}
        </Button>
      {/if}
    {/snippet}
  </Notice>
{:else if m.status === 'permissions'}
  <Notice tone="warn" icon="key" role="status" title={t('mods.needsPerms')}>
    <p class="muted">{t('mods.needsPermsBody', { name })}</p>
  </Notice>
{/if}

{#if m.update}
  <Notice icon="upd" title={t('mods.updateAvailable', { v: m.update })}>
    <p class="muted">{m.update_asks_more ? t('mods.updateAsksMore') : t('mods.updateQuiet')}</p>
    {#snippet actions()}<Button size="sm" variant="primary" icon="download" onclick={() => mods.update(m)}>{t('mods.updateTo', { v: m.update! })}</Button>{/snippet}
  </Notice>
{/if}

{#if m.native}
  <DangerBlock>
    <p>{t('mods.nativeBody', { name })}{#if m.native_allowed_at}{' '}{t('mods.allowedOn', { date: shortDate(m.native_allowed_at) })}{/if}</p>
    {#if !m.reviewed}<p class="muted">{t('mods.notReviewedBody')}</p>{/if}
  </DangerBlock>
{/if}

<section class="card sect">
  <h3>{t('mods.whatItDoes')}</h3>
  {#if modText(m.description)}<p>{modText(m.description)}</p>{/if}
  <dl class="kv">
    <dt>{t('mods.licence')}</dt><dd>{m.license || t('mods.unknown')}</dd>
    <dt>{t('mods.homepage')}</dt>
    <dd>{#if m.homepage}<button class="lnk" onclick={() => api.openUrl(m.homepage!)}>{m.homepage.replace(/^https?:\/\//, '')}</button>{:else}—{/if}</dd>
    <dt>{t('mods.file')}</dt><dd class="mono">{m.file}</dd>
  </dl>
</section>

<section class="card sect">
  <h3>{t('mods.asksFor')}</h3>
  <ul class="perms">
    {#each m.permissions as p (p)}
      {@const info = permInfo(p)}
      {@const ok = m.granted.includes(p)}
      <li class="prow">
        <span class={['pi', info.tier === 1 && 'l1', info.tier === 2 && 'l2', info.tier === 'native' && 'nat']}><Icon name={info.icon} /></span>
        <div class="pb">
          <p class="pt">{t(permKey(info, '.title'), { host: info.host ?? '', name: p })}{#if info.tier === 'native'} <DangerBadge variant="word" />{/if}</p>
          <p class="muted pd">{t(permKey(info, '.desc'), { host: info.host ?? '', name: p })}</p>
        </div>
        <div class="pa">
          {#if info.tier === 'native'}
            {#if ok}
              <Badge tone="bad" icon="check">{m.native_allowed_at ? t('mods.allowedByYouOn', { date: shortDate(m.native_allowed_at) }) : t('mods.allowed')}</Badge>
              <Button size="sm" variant="danger-quiet" disabled={busy} onclick={() => mods.grant(m.id, p, false)}>{t('mods.revokeNative')}</Button>
            {:else}
              <Badge tone="neutral">{t('mods.notAllowed')}</Badge>
            {/if}
          {:else if ok}
            <Badge tone="ok" icon="check">{t('mods.allowed')}</Badge>
            <Button size="sm" disabled={busy} onclick={() => mods.grant(m.id, p, false)}>{t('mods.revoke')}</Button>
          {:else}
            <Badge tone="warn">{t('mods.notAllowed')}</Badge>
            <Button size="sm" variant="primary" disabled={busy} onclick={() => mods.grant(m.id, p, true)}>{t('mods.allow')}</Button>
          {/if}
        </div>
      </li>
    {:else}
      <li class="prow">
        <span class="pi"><Icon name="shield-o" /></span>
        <div class="pb"><p class="pt">{t('perm.none.title')}</p><p class="muted pd">{t('perm.none.desc')}</p></div>
      </li>
    {/each}
  </ul>
  {#if m.native && !m.granted.includes('native')}<p class="hint">{t('mods.nativeReinstall')}</p>{/if}
</section>

<div class="grid">
  <section class="card sect">
    <h3>{t('mods.limits')}</h3>
    <p class="hint">{m.native ? t('mods.limitsNative') : t('mods.limitsIdle')}</p>
    <p class="hint">{t('mods.limitsRule')}</p>
  </section>
  <section class="card sect">
    <h3>{t('mods.needs')}</h3>
    <ul class="needs">
      <li><ModIcon id="api" name="Blockheads API" size="sm" /><span class="nn">{t('mods.api')}<span class="mono hint sp">{m.api}</span></span><Badge tone="ok" icon="check">{t('mods.builtIn')}</Badge></li>
      {#each m.dependencies as d (d.id)}
        <li>
          <ModIcon id={d.id} name={d.id} size="sm" />
          <span class="nn">{d.id}<span class="mono hint sp">{d.req}</span>{#if d.soft}<span class="hint sp">{`· ${t('mods.optional')}`}</span>{/if}</span>
          {#if d.ok}<Badge tone="ok" icon="check">{t('mods.depOk', { v: String(d.installed) })}</Badge>
          {:else if d.installed}<Badge tone="warn" icon="alert">{t('mods.depWrong', { v: d.installed })}</Badge>
          {:else}<Badge tone={d.soft ? 'neutral' : 'bad'} icon="x-circle">{t('mods.depMissing')}</Badge>{/if}
        </li>
      {/each}
    </ul>
  </section>
</div>

<section class="card sect">
  <h3>{t('mods.log')}</h3>
  <pre class="log" role="log" aria-label={t('mods.logOf', { name })}>{t('mods.logEmpty')}</pre>
</section>

{#if confirmRemove}
  <Dialog title={t('mods.removeTitle', { name })} icon="trash" tone="bad" onclose={() => (confirmRemove = false)}>
    <p class="muted">{t('mods.removeBody')}</p>
    {#snippet footer()}
      <Button variant="ghost" onclick={() => (confirmRemove = false)}>{t('common.cancel')}</Button>
      <Button variant="danger" icon="trash" onclick={remove}>{t('mods.remove')}</Button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .head { display: flex; gap: 16px; align-items: flex-start; flex-wrap: wrap; }
  .ht { flex: 1; min-width: 220px; display: flex; flex-direction: column; gap: 6px; }
  h2 { font-size: 26px; }
  .ver { margin-left: 0.3em; font-size: 16px; font-weight: 600; color: var(--text-3); }
  .bd { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
  .acts { display: flex; gap: 8px; flex-wrap: wrap; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: 16px; }
  .sect { padding: 16px 18px; display: flex; flex-direction: column; gap: 10px; }
  h3 { font-size: 17px; }
  .kv { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 6px 14px; font-size: 14.5px; margin: 0; }
  .kv dt { color: var(--text-3); font-weight: 700; }
  .kv dd { margin: 0; overflow-wrap: anywhere; }
  .lnk { background: none; border: 0; padding: 0; color: var(--primary-ink); font: inherit; text-decoration: underline; text-underline-offset: 3px; cursor: pointer; text-align: start; }
  .perms, .needs { list-style: none; margin: 0; padding: 0; }
  .prow { display: flex; gap: 14px; align-items: flex-start; padding: 12px 0; border-top: 1px solid var(--line); flex-wrap: wrap; }
  .prow:first-child { border-top: 0; }
  .pi { width: 36px; height: 36px; border-radius: 10px; display: grid; place-items: center; flex: none; background: var(--surface-2); color: var(--text-2); }
  .pi.l1 { background: var(--warn-soft); color: var(--warn); }
  .pi.l2 { background: var(--bad-soft); color: var(--bad); }
  .pi.nat { background: repeating-linear-gradient(-45deg, var(--mod-danger) 0 6px, var(--mod-danger-stripe) 6px 12px); color: var(--mod-on-danger); }
  .pb { flex: 1; min-width: 200px; }
  .pt { font-weight: 700; }
  .pd { font-size: 14.5px; }
  .pa { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; justify-content: flex-end; }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 14px; }
  .needs li { display: flex; gap: 10px; align-items: center; padding: 6px 0; border-top: 1px solid var(--line); }
  .needs li:first-child { border-top: 0; }
  .sp { margin-left: 0.3em; }
  .nn { flex: 1; font-weight: 700; font-size: 14.5px; }
  .log { background: var(--code-bg); color: var(--code-text); font: 12.5px/1.7 var(--f-mono); padding: 10px 12px; border-radius: 12px; white-space: pre-wrap; margin: 0; }
</style>
