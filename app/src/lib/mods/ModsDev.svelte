<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import ModIcon from './ModIcon.svelte';
  import { mods } from './mods.svelte';
  import Notice from './Notice.svelte';

  const GUIDE = 'https://github.com/Blockheads-Club/mod-sdk#readme';
  let level = $state<'all' | 'errors' | 'warnings'>('all');
  const sel = $derived(mods.dev.find((d) => d.path === mods.devSelected) ?? null);
  /** The log: the folder checks, the launcher's own lines and the running game's mod-loader lines. */
  const levelOf = (s: string) => (/ error /.test(s) ? 'e' : / warn /.test(s) ? 'w' : 'i');
  const lines = $derived([
    ...mods.dev.flatMap((d) => [...d.errors.map((e) => ({ k: 'e', s: `${d.id ?? d.path}: ${e}` })), ...d.warnings.map((w) => ({ k: 'w', s: `${d.id ?? d.path}: ${w}` }))]),
    ...mods.devLog.map((s) => ({ k: 'i', s })),
    ...mods.gameLog.map((s) => ({ k: levelOf(s), s }))
  ]);
  /** The folder mods of the running game, with the time they use. */
  const running = $derived((store.game.mods ?? []).filter((l) => l.origin === 'dev'));
  const errors = $derived(lines.filter((l) => l.k === 'e').length);
  const warnings = $derived(lines.filter((l) => l.k === 'w').length);
  const shown = $derived(lines.filter((l) => level === 'all' || (level === 'errors' ? l.k === 'e' : l.k === 'w')));

  function reload() {
    if (sel) void mods.devReload(sel.path);
  }

  function keydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'r' && store.prefs.mods_developer) {
      e.preventDefault();
      reload();
    }
  }

  let timer: ReturnType<typeof setInterval> | undefined;
  onMount(() => {
    void mods.loadDev();
    timer = setInterval(() => void mods.devWatch(), 1000);
  });
  onDestroy(() => clearInterval(timer));
</script>

<svelte:window onkeydown={keydown} />

<main class="full" aria-label={t('mods.tab.dev')}>
  {#if store.prefs.mods_developer}
    <Notice tone="warn" icon="code" role="note" title={t('mods.dev.on')}>
      <p class="muted">{t('mods.dev.onBody')}</p>
    </Notice>
    <div class="grid">
      <div class="col">
        <section class="card sect">
          <div class="sh"><h3>{t('mods.dev.folders')}</h3><Button size="sm" icon="folder" onclick={() => mods.devAdd()}>{t('mods.dev.load')}</Button></div>
          {#each mods.dev as d (d.path)}
            <button class={['fm', d.path === mods.devSelected && 'on']} aria-pressed={d.path === mods.devSelected} onclick={() => (mods.devSelected = d.path)}>
              <ModIcon id={d.id ?? d.path} name={d.name ?? d.id ?? '?'} size="sm" />
              <span class="fmt">
                <span class="fn">{d.id ?? t('mods.dev.unnamed')}<span class="mono hint v">{d.version}</span></span>
                <span class="mono hint path">{d.path}</span>
                <span class="bd">
                  {#if d.errors.length}<Badge tone="bad" icon="x-circle">{t('mods.dev.problems', { n: d.errors.length })}</Badge>{:else}<Badge tone="ok" icon="check">{t('mods.dev.ok')}</Badge>{/if}
                  {#if d.native}<DangerBadge />{/if}
                </span>
              </span>
            </button>
          {:else}
            <p class="hint">{t('mods.dev.none')}</p>
          {/each}
          {#if sel}
            <div class="acts">
              <Button size="sm" variant="primary" icon="refresh" onclick={reload}>{t('mods.dev.reload')}<span class="kb mono">Ctrl R</span></Button>
              <Button size="sm" icon="x" onclick={() => mods.devRemove(sel.path)}>{t('mods.dev.unload')}</Button>
            </div>
          {/if}
          <Toggle label={t('mods.dev.watch')} checked={store.prefs.mods_dev_watch} onchange={(v) => store.patchPrefs({ mods_dev_watch: v })} />
          <p class="hint">{t('mods.dev.reloadHint')}</p>
        </section>
        <section class="card sect">
          <h3>{t('mods.dev.local')}</h3>
          <p class="hint">{t('mods.dev.localSoon')}</p>
          <Button variant="primary" icon="play" disabled={store.active} onclick={() => mods.devJoin()}>{t('mods.dev.join')}</Button>
          <p class="hint">{t('mods.dev.localHint')}</p>
        </section>
        <section class="card sect">
          <h3>{t('mods.dev.limits')}</h3>
          {#each running as l (l.id)}
            <p class="mono">{`${l.id}: ${t('mods.limitsNow', { ms: l.ms.toFixed(1), peak: l.peak_ms.toFixed(1) })}`}</p>
          {:else}
            <p class="hint">{t('mods.limitsIdle')}</p>
          {/each}
        </section>
      </div>
      <section class="card sect logc">
        <div class="sh">
          <h3>{t('mods.dev.log')}</h3>
          <div class="chips" role="group" aria-label={t('mods.filter')}>
            <button class="chip" aria-pressed={level === 'all'} onclick={() => (level = 'all')}>{t('mods.f.all')}</button>
            <button class="chip" aria-pressed={level === 'errors'} onclick={() => (level = 'errors')}><Icon name="alert" size={14} />{t('mods.dev.errors', { n: errors })}</button>
            <button class="chip" aria-pressed={level === 'warnings'} onclick={() => (level = 'warnings')}>{t('mods.dev.warnings', { n: warnings })}</button>
          </div>
        </div>
        <pre class="log" role="log" aria-live="polite" aria-label={t('mods.dev.logLabel')}>{#each shown as l, i (i)}<span class={l.k}>{l.s}</span>
{:else}{t('mods.dev.logEmpty')}{/each}</pre>
        {#if sel?.errors.length}
          <Notice tone="bad" icon="x-circle" title={sel.errors[0]}>
            {#snippet actions()}
              <Button size="sm" icon="copy" onclick={() => mods.copy(sel.errors.join('\n'))}>{t('mods.copyError')}</Button>
              <Button size="sm" icon="folder" onclick={() => api.openPath(sel.path)}>{t('mods.openFolder')}</Button>
            {/snippet}
          </Notice>
        {/if}
      </section>
    </div>
  {:else}
    <div class="card empty">
      <Icon name="code" />
      <h2>{t('mods.dev.offTitle')}</h2>
      <p class="muted">{t('mods.dev.offBody')}</p>
      <div class="acts">
        <Button variant="primary" icon="code" onclick={() => store.patchPrefs({ mods_developer: true })}>{t('mods.dev.turnOn')}</Button>
        <Button icon="book" onclick={() => api.openUrl(GUIDE)}>{t('mods.dev.guide')}</Button>
      </div>
    </div>
  {/if}
</main>

<style>
  .full { flex: 1; min-width: 0; overflow: auto; padding: 20px 24px 32px; display: flex; flex-direction: column; gap: 16px; }
  .full > :global(*) { flex: none; }
  .grid { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1.5fr); gap: 14px; align-items: start; }
  :global(.narrow) .grid { grid-template-columns: minmax(0, 1fr); }
  .col { display: flex; flex-direction: column; gap: 14px; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: 16px; }
  .sect { padding: 16px 18px; display: flex; flex-direction: column; gap: 10px; }
  .sect h3 { font-size: 17px; }
  .sh { display: flex; justify-content: space-between; align-items: center; gap: 8px; flex-wrap: wrap; }
  .fm { display: flex; gap: 12px; align-items: center; padding: 10px; border-radius: 12px; border: 2px solid var(--line); background: var(--surface); color: inherit; font: inherit; text-align: start; cursor: pointer; }
  .fm.on { border-color: var(--primary); }
  .fmt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .fn { font-weight: 700; }
  .v { font-weight: 400; margin-left: 0.3em; }
  .path { font-size: 12px; overflow-wrap: anywhere; }
  .bd { display: flex; gap: 4px; flex-wrap: wrap; margin-top: 4px; }
  .acts { display: flex; gap: 8px; flex-wrap: wrap; }
  .kb { margin-left: 4px; font-size: 11px; padding: 2px 5px; border: 1.5px solid currentColor; border-radius: 5px; opacity: 0.85; }
  .logc { min-height: 420px; }
  .chips { display: flex; gap: 6px; flex-wrap: wrap; }
  .chip { display: inline-flex; align-items: center; gap: 6px; min-height: 36px; padding: 0 12px; border-radius: 999px; border: 2px solid var(--line); background: var(--surface); color: var(--text-2); font: 700 13.5px var(--f-body); cursor: pointer; }
  .chip[aria-pressed='true'] { background: var(--text); border-color: var(--text); color: var(--bg); }
  .log { flex: 1; min-height: 300px; background: var(--code-bg); color: var(--code-text); font: 12.5px/1.7 var(--f-mono); padding: 10px 12px; border-radius: 12px; white-space: pre-wrap; margin: 0; overflow: auto; }
  .log .w { color: #f2c14e; }
  .log .e { color: #ff8f80; }
  .empty { display: flex; flex-direction: column; align-items: center; text-align: center; gap: 12px; padding: 44px 24px; max-width: 640px; margin: 0 auto; }
  .empty h2 { font-size: 22px; }
</style>
