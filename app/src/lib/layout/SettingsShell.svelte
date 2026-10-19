<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from '../i18n.svelte';
  import { store, type Section } from '../store.svelte';
  import type { IconName } from '../icons';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';

  let { children }: { children: Snippet } = $props();

  const items: { id: string; section: Section; anchor?: string; icon: IconName; label: () => string }[] = [
    { id: 'game', section: 'general', icon: 'file', label: () => t('set.nav.game') },
    { id: 'gfx', section: 'general', anchor: 'sec-gfx', icon: 'monitor', label: () => t('set.nav.gfx') },
    { id: 'controls', section: 'controls', icon: 'mouse', label: () => t('set.nav.controls') },
    { id: 'snd', section: 'general', anchor: 'sec-snd', icon: 'sound', label: () => t('set.nav.audio') },
    { id: 'lng', section: 'general', anchor: 'sec-lng', icon: 'globe', label: () => t('set.nav.lang') },
    { id: 'about', section: 'about', icon: 'info', label: () => t('set.nav.about') }
  ];
  let active = $state('game');
  $effect(() => {
    if (store.section === 'controls') active = 'controls';
    else if (store.section === 'about') active = 'about';
    else if (active === 'controls' || active === 'about') active = 'game';
  });

  function go(item: (typeof items)[number]) {
    active = item.id;
    store.section = item.section;
    if (item.anchor) {
      const anchor = item.anchor;
      queueMicrotask(() => document.getElementById(anchor)?.scrollIntoView({ block: 'start' }));
    }
  }
</script>

<div class="app">
  <header class="top">
    <Button variant="ghost" size="sm" icon="back" label={t('set.back')} onclick={() => store.go('home')} />
    <h1>{t('set.title')}</h1>
    <span class="hint">{t('set.autosave')}</span>
  </header>
  <div class="shell">
    <nav class="snav" aria-label={t('set.navLabel')}>
      {#each items as it (it.id)}
        <button class="item" aria-current={active === it.id ? 'page' : undefined} onclick={() => go(it)}>
          <Icon name={it.icon} />{it.label()}
        </button>
      {/each}
    </nav>
    <main class="smain" id="main" tabindex="-1">{@render children()}</main>
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
  .snav { flex: 1 1 210px; max-width: 250px; padding: 16px 12px; display: flex; flex-direction: column; gap: 4px; border-right: 2px solid var(--line); overflow-y: auto; }
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
  }
  .item:hover { background: var(--surface-2); }
  .item[aria-current='page'] { background: var(--primary-soft); color: var(--primary-ink); }
  .smain { position: relative; flex: 999 1 480px; min-width: 0; padding: 22px 28px 40px; display: flex; flex-direction: column; gap: 16px; outline: none; overflow-y: auto; }
  .smain > :global(*) { max-width: 764px; }
  @media (max-width: 860px) {
    .snav { flex: 1 1 100%; max-width: none; flex-direction: row; overflow-x: auto; border-right: 0; border-bottom: 2px solid var(--line); padding: 8px 12px; }
    .item { white-space: nowrap; }
    .smain { padding: 18px; }
  }
</style>
