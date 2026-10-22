<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { IconName } from '../icons';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import { viewport } from '../viewport.svelte';
  import InstallFlow from './InstallFlow.svelte';
  import { mods, type ModsTab } from './mods.svelte';
  import ModsBrowse from './ModsBrowse.svelte';
  import ModsDev from './ModsDev.svelte';
  import ModsServers from './ModsServers.svelte';
  import MyMods from './MyMods.svelte';
  import { watchPad } from './gamepad';

  const tabs = $derived<{ id: ModsTab; icon: IconName; label: string; n?: string }[]>([
    { id: 'mine', icon: 'puzzle', label: t('mods.tab.mine'), n: mods.overview ? String(mods.mods.length) : undefined },
    { id: 'browse', icon: 'search', label: t('mods.tab.browse') },
    { id: 'servers', icon: 'server', label: t('mods.tab.servers'), n: mods.servers?.servers.length ? String(mods.servers.servers.length) : undefined },
    ...(store.prefs.mods_developer ? [{ id: 'dev' as const, icon: 'code' as const, label: t('mods.tab.dev') }] : [])
  ]);
  const narrow = $derived(viewport.modsNarrow);
  const uid = $props.id();
  let bar = $state<HTMLElement>();

  /** Tabs: arrows move between them (and select), as in the design's tab model. */
  async function tabKey(e: KeyboardEvent) {
    const i = tabs.findIndex((x) => x.id === mods.tab);
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    await go(tabs[(i + step + tabs.length) % tabs.length].id);
  }

  async function go(id: ModsTab) {
    mods.setTab(id);
    await tick();
    bar?.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
  }

  /** Esc: back out of a detail view, else to Home (dialogs handle their own Esc). */
  function keydown(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented || store.dialog) return;
    // The install dialog when the focus is outside it (its own Esc handling stops the event otherwise).
    if (mods.install) return mods.close();
    const tag = (e.target as HTMLElement | null)?.tagName;
    if (tag === 'INPUT' && (e.target as HTMLInputElement).value) return;
    e.preventDefault();
    if (mods.bDetail) mods.bDetail = null;
    else if (narrow && mods.showDetail) mods.showDetail = false;
    else store.go('home');
  }

  onMount(() => {
    void mods.load();
    if (!mods.servers) void mods.loadServers();
    // Controller: LB/RB switch tabs, B goes back (see gamepad.ts for the rest).
    return watchPad({
      tab: (step) => void go(tabs[(tabs.findIndex((x) => x.id === mods.tab) + step + tabs.length) % tabs.length].id)
    });
  });
</script>

<svelte:window onkeydown={keydown} />

<div class={['mods', narrow && 'narrow']}>
  <header class="mtop">
    <Button size="sm" variant="ghost" icon="chev-l" onclick={() => store.go('home')}>{t('tb.home')}</Button>
    {#if !narrow}<span class="ttl"><Icon name="puzzle" />{t('mods.title')}</span>{/if}
    <div class="tabs" role="tablist" aria-label={t('mods.sections')} tabindex="-1" bind:this={bar} onkeydown={tabKey}>
      {#each tabs as tb (tb.id)}
        <button
          class="tab"
          role="tab"
          id={`${uid}-${tb.id}`}
          aria-selected={mods.tab === tb.id}
          aria-controls={`${uid}-panel`}
          tabindex={mods.tab === tb.id ? 0 : -1}
          onclick={() => go(tb.id)}
        ><Icon name={tb.icon} size={16} />{`${tb.label} `}{#if tb.n}<span class="n">{tb.n}</span>{/if}</button>
      {/each}
    </div>
    <span class="sp"></span>
    {#if mods.tab !== 'dev'}
      {#if narrow}
        <Button size="sm" icon="zip" label={t('mods.installFile')} onclick={() => mods.startFile()} />
      {:else}
        <Button size="sm" icon="zip" onclick={() => mods.startFile()}>{t('mods.installFile')}</Button>
      {/if}
    {/if}
  </header>

  {#if mods.tab === 'mine' || mods.tab === 'browse'}
    <p class="soon" role="note"><Icon name="clock" size={16} /><span><strong>{t('mods.soon')}</strong> {t('mods.soonBody')}</span></p>
  {/if}

  <div class="panel" id={`${uid}-panel`} role="tabpanel" aria-labelledby={`${uid}-${mods.tab}`} tabindex="-1">
    {#if mods.tab === 'mine'}<MyMods {narrow} />
    {:else if mods.tab === 'browse'}<ModsBrowse />
    {:else if mods.tab === 'servers'}<ModsServers />
    {:else}<ModsDev />{/if}
  </div>
</div>

<InstallFlow />

<style>
  .mods { height: 100%; display: flex; flex-direction: column; overflow: hidden; background: var(--bg); }
  .mtop {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    border-bottom: 2px solid var(--line);
    background: var(--surface);
    flex: none;
    flex-wrap: wrap;
  }
  .ttl { font: 800 19px var(--f-display); display: flex; align-items: center; gap: 8px; }
  .sp { flex: 1; }
  .tabs { display: inline-flex; gap: 4px; padding: 4px; background: var(--surface-2); border: 2px solid var(--line); border-radius: 14px; flex-wrap: wrap; }
  .tab {
    min-height: 40px;
    padding: 0 14px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--text-2);
    font: 700 14.5px/1 var(--f-body);
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    white-space: nowrap;
  }
  .narrow .tab { padding: 0 10px; }
  .tab[aria-selected='true'] { background: var(--surface); color: var(--text); box-shadow: 0 2px 0 var(--line-strong); }
  .n { font: 600 12px var(--f-mono); color: var(--text-3); }
  .soon {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 8px 16px;
    font-size: 14px;
    background: var(--sun-soft);
    color: var(--sun-ink);
    border-bottom: 2px solid var(--line);
    flex: none;
  }
  .soon :global(.ico) { flex: none; }
  .panel { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .panel:focus { outline: none; }
</style>
