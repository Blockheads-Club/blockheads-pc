<script lang="ts">
  import { api } from '../api';
  import { bytes } from '../format';
  import { t } from '../i18n.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import Badges from './Badges.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import ModIcon from './ModIcon.svelte';
  import { errorText, mods, sidesOf } from './mods.svelte';
  import Notice from './Notice.svelte';
  import PermissionChip from './PermissionChip.svelte';
  import { permInfo, permKey } from './perms';
  import SidesBadge from './SidesBadge.svelte';
  import type { BrowseMod } from './types';

  const b = $derived(mods.browse);
  const list = $derived(mods.browsed);
  const detail = $derived(b?.mods.find((m) => m.id === mods.bDetail) ?? null);
  const offline = $derived(mods.browseError?.code === 'offline' || mods.browseError?.code === 'index' ? mods.browseError : null);
  const REPORT_URL = 'https://github.com/Blockheads-Club/mods/issues/new';

  /** What the card's button does: install, update, or nothing (installed). */
  function state(m: BrowseMod): 'install' | 'update' | 'installed' {
    if (!m.installed) return 'install';
    return m.installed === m.version || m.installed === m.compatible_version ? 'installed' : 'update';
  }

  function act(m: BrowseMod) {
    void mods.installFromIndex(m.id, null);
  }

  function clear() {
    mods.bQuery = '';
    mods.bTag = null;
    mods.bHideNative = false;
  }
</script>

{#snippet action(m: BrowseMod, big: boolean)}
  {@const s = state(m)}
  {#if s === 'installed'}
    <Badge tone="ok" icon="check">{t('mods.installed')}</Badge>
  {:else if s === 'update'}
    <Button size={big ? 'lg' : 'sm'} icon="upd" onclick={() => act(m)}>{t('mods.update')}</Button>
  {:else}
    <Button size={big ? 'lg' : 'sm'} variant={m.native ? 'danger' : 'primary'} icon="download" label={big ? undefined : t('mods.installName', { name: m.name })} onclick={() => act(m)}>{t('mods.install')}</Button>
  {/if}
{/snippet}

<main class="full" aria-label={t('mods.tab.browse')}>
  {#if detail}
    <button class="backlink" onclick={() => (mods.bDetail = null)}><Icon name="chev-l" size={16} />{t('mods.tab.browse')}</button>
    <div class="dhead">
      <ModIcon id={detail.id} name={detail.name} size="lg" />
      <div class="ht">
        <h1>{detail.name}<span class="mono ver">{detail.version}</span></h1>
        <div class="bd">
          <SidesBadge sides={sidesOf(detail.sides)} />
          {#if detail.native}<DangerBadge />{/if}
          <Badges kind="reviewed" />
          <span class="hint">{[t('mods.by', { who: detail.owners.join(', ') }), detail.published && t('mods.updatedOn', { date: detail.published })].filter(Boolean).join(' · ')}</span>
        </div>
      </div>
      {@render action(detail, true)}
    </div>
    {#if !detail.compatible}
      <Notice tone="warn" icon="upd" title={t('mods.newer.title')}>
        <p class="muted">{detail.compatible_version ? t('mods.newer.short', { v: detail.compatible_version }) : t('mods.newer.none')}</p>
      </Notice>
    {/if}
    <div class="dgrid">
      <div class="col">
        <section class="card sect">
          <h3>{t('mods.about')}</h3>
          {#if detail.description}<p>{detail.description}</p>{/if}
          <dl class="kv">
            <dt>{t('mods.homepage')}</dt>
            <dd><button class="lnk" onclick={() => api.openUrl(detail.repo)}>{detail.repo.replace(/^https?:\/\//, '')}</button></dd>
            {#if detail.size}<dt>{t('mods.size')}</dt><dd>{bytes(detail.size)}</dd>{/if}
            {#if detail.tags.length}<dt>{t('mods.tags')}</dt><dd>{detail.tags.join(', ')}</dd>{/if}
          </dl>
        </section>
        <section class="card sect">
          <h3>{t('mods.asksFor')}</h3>
          <ul class="perms">
            {#each detail.permissions as p (p)}
              {@const info = permInfo(p)}
              <li class="prow">
                <span class={['pi', info.tier === 'native' && 'nat']}><Icon name={info.icon} /></span>
                <div><p class="pt">{t(permKey(info, '.title'), { host: info.host ?? '', name: p })}</p><p class="muted pd">{t(permKey(info, '.desc'), { host: info.host ?? '', name: p })}</p></div>
              </li>
            {:else}
              <li class="prow"><span class="pi"><Icon name="shield-o" /></span><div><p class="pt">{t('perm.none.title')}</p><p class="muted pd">{t('perm.none.desc')}</p></div></li>
            {/each}
          </ul>
          <p class="hint">{t('mods.chooseEach')}</p>
        </section>
      </div>
      <div class="col">
        <section class="card sect">
          <h3>{t('mods.needs')}</h3>
          <p class="need"><ModIcon id="api" name="Blockheads API" size="sm" /><span class="nn">{t('mods.api')}</span><Badge tone="ok">{t('mods.builtIn')}</Badge></p>
          {#each Object.entries(detail.depends) as [id, req] (id)}
            <p class="need"><ModIcon {id} name={id} size="sm" /><span class="nn">{id}<span class="mono hint sp">{req}</span></span></p>
          {/each}
        </section>
        <section class="card sect">
          <h3>{t('mods.versions')}</h3>
          <ul class="vers">{#each detail.versions as [v, date] (v)}<li><strong>{v}</strong>{#if date}{` — ${date}`}{/if}</li>{/each}</ul>
        </section>
        <button class="card sect report" onclick={() => api.openUrl(REPORT_URL)}><Icon name="alert" /><span>{t('mods.report')}</span><Icon name="chev-r" /></button>
      </div>
    </div>
  {:else}
    <div class="tools">
      <div class="srch" role="search">
        <Icon name="search" />
        <label class="sr" for="mods-bq">{t('mods.b.search')}</label>
        <input id="mods-bq" class="input" placeholder={t('mods.b.searchPh')} bind:value={mods.bQuery} />
      </div>
      <div class="seg" role="group" aria-label={t('mods.b.sides')}>
        <button aria-pressed={!mods.bAll} onclick={() => (mods.bAll = false)}>{t('mods.b.players')}</button>
        <button aria-pressed={mods.bAll} onclick={() => (mods.bAll = true)}>{t('mods.b.all')}</button>
      </div>
      <Toggle label={t('mods.b.hideNative')} checked={mods.bHideNative} onchange={(v) => (mods.bHideNative = v)} />
    </div>
    {#if b?.tags.length}
      <div class="chips" role="group" aria-label={t('mods.tags')}>
        <button class="chip" aria-pressed={mods.bTag === null} onclick={() => (mods.bTag = null)}>{t('mods.f.all')}</button>
        {#each b.tags as tag (tag)}
          <button class="chip" aria-pressed={mods.bTag === tag} onclick={() => (mods.bTag = tag)}>{tag}</button>
        {/each}
      </div>
    {/if}

    {#if offline}
      <Notice tone="warn" icon="wifi-off" role="alert" title={t('mods.b.offline')}>
        <p class="muted">{t('mods.b.offlineBody')}</p>
        {#snippet actions()}
          <Button size="sm" variant="primary" icon="refresh" onclick={() => mods.loadBrowse(true)}>{t('common.retry')}</Button>
          <Button size="sm" icon="zip" onclick={() => mods.startFile()}>{t('mods.installFileShort')}</Button>
        {/snippet}
      </Notice>
    {:else if mods.browseError}
      <Notice tone="bad" icon="alert" role="alert" title={t('mods.b.error')}>
        <p class="muted">{errorText(mods.browseError)}</p>
        {#snippet actions()}<Button size="sm" variant="primary" icon="refresh" onclick={() => mods.loadBrowse(true)}>{t('common.retry')}</Button>{/snippet}
      </Notice>
    {/if}

    {#if b?.offline}
      <p class="hint" role="status"><Icon name="wifi-off" size={14} /> {t('mods.b.cached', { date: b.generated.slice(0, 10) })}</p>
    {/if}

    {#if !b && mods.browseLoading}
      <div class="grid" aria-busy="true">
        {#each [1, 2, 3, 4, 5, 6] as i (i)}
          <div class="card sk"><div class="skrow"><span class="skel" style="width:44px;height:44px"></span><div class="skcol"><span class="skel" style="height:15px;width:60%"></span><span class="skel" style="height:12px;width:40%"></span></div></div><span class="skel" style="height:12px"></span><span class="skel" style="height:22px;width:70%"></span></div>
        {/each}
      </div>
      <p class="sr" role="status">{t('mods.b.loading')}</p>
    {:else if b}
      {#if list.length}
        <p class="hint" role="status">{t('mods.b.count', { n: list.length, total: b.mods.length })} · <Icon name="shield-ok" size={14} /> {t('mods.b.allReviewed')}</p>
        <div class="grid">
          {#each list as m (m.id)}
            <article class={['card', 'mc', m.native && 'dz']}>
              {#if m.native}<div class="dz-h"><Icon name="octagon" size={16} /><span>{t('mods.b.nativeBand')}</span></div>{/if}
              <div class="mcb">
                <div class="mch">
                  <ModIcon id={m.id} name={m.name} />
                  <div class="mct">
                    <h3><button class="lnk name" onclick={() => (mods.bDetail = m.id)}>{m.name}</button></h3>
                    <p class="hint">{[t('mods.by', { who: m.owners.join(', ') }), ...m.tags].join(' · ')}</p>
                  </div>
                  {@render action(m, false)}
                </div>
                {#if m.description}<p class="muted desc">{m.description}</p>{/if}
                <div class="bd">
                  <SidesBadge sides={sidesOf(m.sides)} />
                  {#each m.permissions as p (p)}<PermissionChip perm={p} />{:else}<span class="perm0"><Icon name="shield-o" size={14} />{t('perm.none.title')}</span>{/each}
                </div>
                {#if m.sides.includes('server') && m.sides.includes('client')}<p class="hint"><Icon name="server" size={14} />{` ${t('mods.b.needsServer')}`}</p>{/if}
                {#if !m.sides.includes('client')}<p class="hint"><Icon name="server" size={14} />{` ${t('mods.b.serverOnly')}`}</p>{/if}
                <p class="hint foot"><Badges kind="reviewed" />{#if m.size}<span>{bytes(m.size)}</span>{/if}</p>
              </div>
            </article>
          {/each}
        </div>
      {:else}
        <div class="card empty">
          <Icon name="search" />
          <h2>{mods.bQuery ? t('mods.b.none', { q: mods.bQuery }) : b.mods.length ? t('mods.b.noneFilter') : t('mods.b.emptyList')}</h2>
          <p class="muted">{t('mods.b.noneBody')}</p>
          {#if b.mods.length}<Button size="sm" onclick={clear}>{t('mods.b.clear')}</Button>{/if}
        </div>
      {/if}
      <p class="hint">{t('mods.b.source')}</p>
    {/if}
  {/if}
</main>

<style>
  .full { flex: 1; min-width: 0; overflow: auto; padding: 20px 24px 32px; display: flex; flex-direction: column; gap: 16px; }
  .full > :global(*) { flex: none; }
  .tools { display: flex; gap: 10px; flex-wrap: wrap; align-items: center; }
  .srch { position: relative; flex: 1 1 240px; min-width: 0; }
  .srch > :global(.ico) { position: absolute; left: 12px; top: 12px; color: var(--text-3); pointer-events: none; }
  .input { min-height: 44px; padding: 0 14px 0 42px; border: 2px solid var(--line-strong); border-radius: 10px; background: var(--surface); color: var(--text); font: 15px var(--f-body); width: 100%; }
  .seg { display: inline-flex; border: 2px solid var(--line-strong); border-radius: 10px; overflow: hidden; flex: none; }
  .seg button { min-height: 40px; padding: 0 12px; border: 0; background: var(--surface); color: var(--text-2); font: 700 13.5px var(--f-body); cursor: pointer; }
  .seg button + button { border-left: 2px solid var(--line-strong); }
  .seg button[aria-pressed='true'] { background: var(--text); color: var(--bg); }
  .chips { display: flex; gap: 6px; flex-wrap: wrap; }
  .chip { display: inline-flex; align-items: center; min-height: 36px; padding: 0 12px; border-radius: 999px; border: 2px solid var(--line); background: var(--surface); color: var(--text-2); font: 700 13.5px var(--f-body); cursor: pointer; }
  .chip[aria-pressed='true'] { background: var(--text); border-color: var(--text); color: var(--bg); }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 12px; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: 16px; }
  .mc { display: flex; flex-direction: column; overflow: hidden; }
  .mc.dz { border-color: var(--mod-danger-edge); }
  .dz-h {
    display: flex; align-items: center; gap: 8px; padding: 6px 12px; color: var(--mod-on-danger); font: 800 12.5px var(--f-body);
    letter-spacing: 0.04em; text-transform: uppercase;
    background: repeating-linear-gradient(-45deg, var(--mod-danger) 0 12px, var(--mod-danger-stripe) 12px 24px);
  }
  .mcb { padding: 14px 16px; display: flex; flex-direction: column; gap: 8px; flex: 1; }
  .mch { display: flex; gap: 12px; align-items: center; }
  .mct { flex: 1; min-width: 0; }
  .mct h3 { font-size: 17px; }
  .lnk { background: none; border: 0; padding: 0; color: var(--primary-ink); font: inherit; text-decoration: underline; text-underline-offset: 3px; cursor: pointer; text-align: start; }
  .lnk.name { color: var(--text); text-decoration: none; font-weight: 750; }
  .lnk.name:hover { text-decoration: underline; }
  .desc { font-size: 14.5px; }
  .bd { display: flex; gap: 4px; flex-wrap: wrap; align-items: center; }
  .perm0 { display: inline-flex; align-items: center; gap: 6px; padding: 2px 9px 2px 7px; border-radius: 7px; font: 700 12.5px/1.5 var(--f-body); background: var(--surface-2); color: var(--text-2); border: 1.5px solid var(--line); }
  .foot { display: flex; gap: 10px; flex-wrap: wrap; margin-top: auto; }
  .empty { display: flex; flex-direction: column; align-items: center; text-align: center; gap: 12px; padding: 44px 24px; }
  .empty h2 { font-size: 21px; }
  .sk { padding: 14px 16px; display: flex; flex-direction: column; gap: 8px; }
  .skrow { display: flex; gap: 12px; }
  .skcol { flex: 1; display: flex; flex-direction: column; gap: 6px; }
  .skel { display: block; border-radius: 8px; background: linear-gradient(90deg, var(--surface-2), var(--surface-3), var(--surface-2)); background-size: 200% 100%; animation: sk 1.4s ease-in-out infinite; }
  @keyframes sk { to { background-position: -200% 0; } }
  @media (prefers-reduced-motion: reduce) { .skel { animation: none; } }
  .backlink { align-self: flex-start; display: inline-flex; gap: 4px; align-items: center; font: 700 14px var(--f-body); color: var(--primary-ink); background: none; border: 0; padding: 4px 0; cursor: pointer; }
  .dhead { display: flex; gap: 16px; align-items: flex-start; flex-wrap: wrap; }
  .ht { flex: 1; min-width: 240px; display: flex; flex-direction: column; gap: 6px; }
  h1 { font-size: 28px; }
  .ver { margin-left: 0.3em; font-size: 17px; color: var(--text-3); }
  .dgrid { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr); gap: 14px; align-items: start; }
  :global(.narrow) .dgrid { grid-template-columns: minmax(0, 1fr); }
  .col { display: flex; flex-direction: column; gap: 14px; }
  .sect { padding: 16px 18px; display: flex; flex-direction: column; gap: 10px; }
  .sect h3 { font-size: 17px; }
  .kv { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 6px 14px; font-size: 14.5px; margin: 0; }
  .kv dt { color: var(--text-3); font-weight: 700; }
  .kv dd { margin: 0; overflow-wrap: anywhere; }
  .perms { list-style: none; margin: 0; padding: 0; }
  .prow { display: flex; gap: 14px; align-items: flex-start; padding: 12px 0; border-top: 1px solid var(--line); }
  .prow:first-child { border-top: 0; }
  .pi { width: 36px; height: 36px; border-radius: 10px; display: grid; place-items: center; flex: none; background: var(--surface-2); color: var(--text-2); }
  .pi.nat { background: repeating-linear-gradient(-45deg, var(--mod-danger) 0 6px, var(--mod-danger-stripe) 6px 12px); color: var(--mod-on-danger); }
  .pt { font-weight: 700; }
  .pd { font-size: 14.5px; }
  .need { display: flex; gap: 10px; align-items: center; }
  .sp { margin-left: 0.3em; }
  .nn { flex: 1; font-weight: 700; }
  .vers { margin: 0; padding-left: 18px; font-size: 14.5px; display: flex; flex-direction: column; gap: 4px; }
  .report { flex-direction: row; align-items: center; cursor: pointer; color: var(--text); font: 700 14.5px var(--f-body); text-align: start; }
  .report span { flex: 1; }
</style>
