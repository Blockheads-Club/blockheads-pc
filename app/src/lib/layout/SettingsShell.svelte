<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { t } from '../i18n.svelte';
  import { PAGES, searchSettings, type SettingEntry, type SettingsPage } from '../settings-index';
  import { store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';

  let { children }: { children: Snippet } = $props();

  let query = $state('');
  let input = $state<HTMLInputElement>();
  let main = $state<HTMLElement>();
  let list = $state<HTMLElement>();
  const hits = $derived(searchSettings(query));
  const searching = $derived(query.trim() !== '');
  const groups = $derived(PAGES.map((p) => ({ ...p, items: hits.filter((h) => h.page === p.id) })).filter((g) => g.items.length));
  const uid = $props.id();

  /** The first thing in a setting's row that takes the keyboard: its switch, list, slider, or button. */
  const CONTROL = 'input:not(:disabled), select:not(:disabled), button:not(:disabled):not([tabindex="-1"])';

  async function show(page: SettingsPage) {
    query = '';
    store.section = page;
    await tick();
    main!.scrollTop = 0;
  }

  async function go(page: SettingsPage) {
    await show(page);
    main!.focus({ preventScroll: true });
  }

  /** Open a search result: its page, scrolled to the setting, highlighted for a moment, its control focused. */
  async function open(hit: SettingEntry) {
    await show(hit.page);
    const row = document.getElementById(hit.id) ?? (hit.fallback ? document.getElementById(hit.fallback) : null);
    if (!row) return main!.focus({ preventScroll: true });
    const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
    row.scrollIntoView({ block: 'center', behavior: still ? 'auto' : 'smooth' });
    row.classList.remove('flash');
    void row.offsetWidth;
    row.classList.add('flash');
    setTimeout(() => row.classList.remove('flash'), 1800);
    const target = row.querySelector<HTMLElement>(CONTROL) ?? row;
    if (target === row) row.tabIndex = -1;
    target.focus({ preventScroll: true });
  }

  const resultButtons = () => [...list!.querySelectorAll<HTMLElement>('button.res')];

  function searchKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && query) {
      e.preventDefault();
      e.stopPropagation();
      query = '';
    } else if (e.key === 'ArrowDown' && hits.length) {
      e.preventDefault();
      resultButtons()[0].focus();
    } else if (e.key === 'Enter' && hits.length) {
      e.preventDefault();
      void open(groups[0].items[0]);
    }
  }

  /** Arrow keys walk the result list; up from the first result returns to the search field, Esc clears. */
  function listKey(e: KeyboardEvent) {
    const buttons = resultButtons();
    const i = buttons.indexOf(e.target as HTMLElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = buttons[i + (e.key === 'ArrowDown' ? 1 : -1)];
      (next ?? (e.key === 'ArrowUp' ? input! : buttons[i])).focus();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      query = '';
      input!.focus();
    }
  }

  /** Ctrl+F (or Cmd+F) and "/" jump to the search field, unless a dialog is open or "/" is being typed. */
  function windowKey(e: KeyboardEvent) {
    if (e.defaultPrevented || document.querySelector('[role="dialog"]')) return;
    const find = (e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'f';
    const typing = (e.target as HTMLElement).closest('input, select, textarea, [contenteditable]');
    if (find || (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey && !e.altKey)) {
      e.preventDefault();
      input!.focus();
      input!.select();
    }
  }
</script>

<svelte:window onkeydown={windowKey} />

<div class="app">
  <header class="top">
    <Button variant="ghost" size="sm" icon="back" label={t('set.back')} onclick={() => store.go('home')} />
    <h1>{t('set.title')}</h1>
    <span class="hint">{t('set.autosave')}</span>
  </header>
  <div class="shell">
    <div class="side">
      <div class="search" role="search">
        <span class="sicon" aria-hidden="true"><Icon name="search" size={18} /></span>
        <input
          bind:this={input}
          bind:value={query}
          type="search"
          placeholder={t('set.search.label')}
          aria-label={t('set.search.label')}
          aria-keyshortcuts="Control+F /"
          aria-controls="main"
          autocomplete="off"
          spellcheck="false"
          onkeydown={searchKey}
        />
        {#if query}
          <button class="clear" aria-label={t('set.search.clear')} onclick={() => { query = ''; input!.focus(); }}><Icon name="x" size={16} /></button>
        {/if}
      </div>
      <nav class="snav" aria-label={t('set.navLabel')}>
        {#each PAGES as p (p.id)}
          {@const n = hits.filter((h) => h.page === p.id).length}
          <button class={['item', searching && !n && 'dim']} aria-current={!searching && store.section === p.id ? 'page' : undefined} onclick={() => go(p.id)}>
            <Icon name={p.icon} /><span class="lbl">{t(p.labelKey)}</span>
            {#if searching && n}<span class="cnt" aria-label={t('set.search.matches', { n })}>{n}</span>{/if}
          </button>
        {/each}
      </nav>
    </div>
    <main class="smain" id="main" tabindex="-1" bind:this={main}>
      {#if searching}
        <div class="results" bind:this={list} role="region" aria-label={t('set.search.results')}>
          <p class="hint" role="status">{t('set.search.count', { n: hits.length })}</p>
          {#if groups.length}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="groups" onkeydown={listKey}>
              {#each groups as g (g.id)}
                <section class="grp" aria-labelledby={`${uid}-${g.id}`}>
                  <h2 id={`${uid}-${g.id}`}><Icon name={g.icon} size={18} />{t(g.labelKey)}</h2>
                  <ul>
                    {#each g.items as hit (hit.id)}
                      <li>
                        <button class="res" onclick={() => open(hit)}>
                          <span class="rt">{t(hit.titleKey)}</span>
                          <span class="rh">{t(hit.hintKey)}</span>
                          <span class="go" aria-hidden="true"><Icon name="chev-r" size={18} /></span>
                        </button>
                      </li>
                    {/each}
                  </ul>
                </section>
              {/each}
            </div>
          {:else}
            <div class="empty">
              <span class="eicon" aria-hidden="true"><Icon name="search" size={28} /></span>
              <p class="et">{t('set.search.none', { q: query.trim() })}</p>
              <p class="hint">{t('set.search.noneHint')}</p>
            </div>
          {/if}
        </div>
      {:else}
        {@render children()}
      {/if}
    </main>
  </div>
</div>

<style>
  /* Fixed-height app layout: the header and the section nav stay put, only the section content scrolls. */
  /* Pinned to the content area (position: relative there), not height: 100%: WebKitGTK does not resolve a
     percentage height against that flex item, and the shell then grew with its content and scrolled as a whole. */
  .app { position: absolute; inset: 0; display: flex; flex-direction: column; overflow: hidden; background: var(--bg); }
  .top { display: flex; align-items: center; gap: 10px; padding: 10px 16px; border-bottom: 2px solid var(--line); background: var(--surface); }
  h1 { font: 800 18px var(--f-display); flex: 1; }
  .shell { flex: 1; min-height: 0; display: flex; }
  .side { flex: 1 1 210px; max-width: 250px; min-height: 0; display: flex; flex-direction: column; border-right: 2px solid var(--line); }
  .search { position: relative; padding: 16px 12px 8px; }
  .search input {
    width: 100%;
    min-height: 42px;
    padding: 0 38px;
    border: 2px solid var(--line-strong);
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--text);
    font: 15px/1.4 var(--f-body);
    appearance: none;
  }
  .search input::placeholder { color: var(--text-3); }
  .search input::-webkit-search-cancel-button { display: none; }
  .search input:focus-visible { outline: 3px solid var(--focus); outline-offset: 1px; border-color: var(--focus); }
  .sicon { position: absolute; left: 24px; top: 28px; display: flex; color: var(--text-3); pointer-events: none; }
  .clear {
    position: absolute;
    right: 18px;
    top: 21px;
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text-3);
    cursor: pointer;
  }
  .clear:hover { background: var(--surface-2); color: var(--text); }
  .snav { flex: 1; min-height: 0; padding: 4px 12px 16px; display: flex; flex-direction: column; gap: 4px; overflow-y: auto; }
  .item {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 44px;
    padding: 0 12px;
    border-radius: 10px;
    border: 0;
    background: transparent;
    color: var(--text-2);
    font: 700 15px var(--f-body);
    cursor: pointer;
    text-align: left;
    transition: opacity var(--dur-fast);
  }
  .item:hover { background: var(--surface-2); }
  .item[aria-current='page'] { background: var(--primary-soft); color: var(--primary-ink); }
  .item.dim { opacity: 0.45; }
  .lbl { flex: 1; }
  .cnt {
    min-width: 22px;
    padding: 1px 7px;
    border-radius: 999px;
    background: var(--primary);
    color: var(--on-primary);
    font: 700 12px/1.5 var(--f-body);
    text-align: center;
  }
  .smain { position: relative; flex: 999 1 480px; min-width: 0; padding: 22px 28px 40px; display: flex; flex-direction: column; gap: 16px; outline: none; overflow-y: auto; }
  .smain > :global(*) { max-width: 764px; }
  .results { display: flex; flex-direction: column; gap: 12px; }
  .groups { display: flex; flex-direction: column; gap: 16px; }
  .grp { background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); padding: 4px 8px 8px; }
  .grp h2 { display: flex; align-items: center; gap: 8px; font-size: 15px; color: var(--text-2); padding: 10px 12px 6px; }
  ul { list-style: none; margin: 0; padding: 0; }
  .res {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: 12px;
    width: 100%;
    padding: 10px 12px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--text);
    font: 15px/1.4 var(--f-body);
    text-align: left;
    cursor: pointer;
  }
  .res:hover, .res:focus-visible { background: var(--surface-2); }
  .rt { font-weight: 700; }
  .rh { grid-column: 1; font-size: 13.5px; color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .go { grid-column: 2; grid-row: 1 / 3; align-self: center; display: flex; color: var(--text-3); }
  .empty { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 48px 16px; text-align: center; }
  .eicon { width: 56px; height: 56px; border-radius: 16px; background: var(--surface-2); color: var(--text-3); display: grid; place-items: center; margin-bottom: 6px; }
  .et { font-weight: 700; font-size: 16px; }
  @media (max-width: 860px) {
    .shell { flex-direction: column; }
    .side { flex: none; max-width: none; border-right: 0; border-bottom: 2px solid var(--line); }
    .search { padding: 10px 12px 0; }
    .sicon { top: 22px; }
    .clear { top: 15px; }
    .snav { flex-direction: row; overflow-x: auto; padding: 8px 12px; }
    .item { white-space: nowrap; }
    .smain { flex: 1; padding: 18px; }
  }
</style>
