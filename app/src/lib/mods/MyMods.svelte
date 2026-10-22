<script lang="ts">
  import { tick } from 'svelte';
  import { t } from '../i18n.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import Badges from './Badges.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import ModDetail from './ModDetail.svelte';
  import ModIcon from './ModIcon.svelte';
  import { errorReport, errorText, modName, mods, PROBLEMS, type MineFilter } from './mods.svelte';
  import ModStatus from './ModStatus.svelte';
  import Notice from './Notice.svelte';

  let { narrow = false }: { narrow?: boolean } = $props();
  let list = $state<HTMLElement>();
  const ov = $derived(mods.overview);
  const cur = $derived(mods.current);
  const filters = $derived<{ id: MineFilter; label: string; n: number }[]>([
    { id: 'all', label: t('mods.f.all'), n: mods.counts.all },
    { id: 'on', label: t('mods.f.on'), n: mods.counts.on },
    { id: 'problems', label: t('mods.f.problems'), n: mods.counts.problems },
    { id: 'updates', label: t('mods.f.updates'), n: mods.counts.updates }
  ]);

  /** Up/Down (and the D-pad) walk the list; the selection follows the focus. */
  async function listKey(e: KeyboardEvent) {
    const rows = [...list!.querySelectorAll<HTMLElement>('button.pick')];
    const i = rows.indexOf(e.target as HTMLElement);
    const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (i < 0 || !step) return;
    e.preventDefault();
    const next = rows[Math.min(Math.max(i + step, 0), rows.length - 1)];
    mods.selected = next.dataset.id!;
    await tick();
    next.focus();
  }

  function open(id: string) {
    mods.select(id);
  }
</script>

{#if mods.error}
  <div class="pad">
    <Notice tone="bad" icon="alert" role="alert" title={t('mods.readError')}>
      <p class="mono path">{mods.error.file ?? ov?.dir ?? ''}</p>
      <p class="muted">{errorText(mods.error)}</p>
      <p class="muted">{t('mods.readErrorBody')}</p>
      {#snippet actions()}
        <Button size="sm" variant="primary" icon="refresh" onclick={() => mods.load()}>{t('common.retry')}</Button>
        <Button size="sm" icon="folder" onclick={() => mods.openFolder()}>{t('mods.openFolder')}</Button>
        <Button size="sm" variant="ghost" icon="copy" onclick={() => mods.copy(errorReport(mods.error!))}>{t('mods.copyError')}</Button>
      {/snippet}
    </Notice>
  </div>
{:else if ov && !ov.mods.length && !ov.problems.length}
  <div class="empty">
    <div class="blk" aria-hidden="true"><span style:background="#D9C7F0" style:transform="rotate(-6deg)"></span><span style:background="#FFD27A" style:transform="translateY(-8px)"></span><span style:background="#B9D7F5" style:transform="rotate(5deg)"></span></div>
    <h2>{t('mods.empty')}</h2>
    <p class="muted">{t('mods.emptyBody')}</p>
    <div class="acts">
      <Button variant="primary" icon="search" onclick={() => mods.setTab('browse')}>{t('mods.browseList')}</Button>
      <Button icon="zip" onclick={() => mods.startFile()}>{t('mods.installFileLong')}</Button>
    </div>
    <p class="hint"><Icon name="shield-ok" size={16} /> {t('mods.reviewedNote')}</p>
  </div>
{:else}
  <div class="mbody">
    {#if !narrow || !mods.showDetail}
      <div class="mlist">
        <div class="tools">
          <div class="srch">
            <Icon name="search" />
            <label class="sr" for="mods-q">{t('mods.search')}</label>
            <input id="mods-q" class="input" placeholder={t('mods.search')} bind:value={mods.query} />
          </div>
          <div class="chips" role="group" aria-label={t('mods.filter')}>
            {#each filters as f (f.id)}
              <button class="chip" aria-pressed={mods.filter === f.id} onclick={() => (mods.filter = f.id)}>
                {#if f.id === 'problems'}<Icon name="alert" size={14} />{/if}{`${f.label} `}<span class="n">{f.n}</span>
              </button>
            {/each}
          </div>
        </div>
        {#if !ov}
          <ul aria-busy="true">
            {#each [150, 120, 170, 140, 110] as w (w)}
              <li class="li"><span class="skel" style="width:32px;height:32px"></span><div class="mm"><span class="skel" style="height:14px" style:width={`${w}px`}></span><span class="skel" style="height:12px" style:width={`${w - 40}px`}></span></div></li>
            {/each}
          </ul>
          <p class="sr" role="status">{t('mods.loading')}</p>
        {:else}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <ul aria-label={t('mods.tab.mine')} bind:this={list} onkeydown={listKey}>
            {#each mods.shown as m (m.id)}
              <li class={['li', PROBLEMS.includes(m.status) && 'att']} data-sel={cur?.id === m.id}>
                <button class="pick" aria-current={cur?.id === m.id} data-id={m.id} tabindex={cur?.id === m.id ? 0 : -1} onclick={() => open(m.id)}>
                  <ModIcon id={m.id} name={modName(m)} size="sm" />
                  <div class="mm">
                    <span class="nm">{modName(m)}<span class="mono hint v">{m.version}</span></span>
                    <span class="bd">
                      <ModStatus status={mods.statusOf(m)} />
                      {#if m.update}<Badges kind="update" version={m.update} />{/if}
                      {#if m.native}<DangerBadge />{/if}
                    </span>
                  </div>
                </button>
                <Toggle label={t('mods.onOff', { name: modName(m) })} hideLabel checked={m.enabled} disabled={mods.busy === m.id} onchange={(v) => mods.toggle(m.id, v)} />
              </li>
            {:else}
              <li class="none muted">{t('mods.noMatch')}</li>
            {/each}
          </ul>
          {#if ov.problems.length}
            <div class="probs" role="note">
              <p class="hint"><Icon name="alert" size={14} /> {t('mods.brokenFiles', { n: ov.problems.length })}</p>
              <ul>{#each ov.problems as p (p.file)}<li class="mono hint">{`${p.file}: ${p.message}`}</li>{/each}</ul>
            </div>
          {/if}
          <p class="hint foot">{t('mods.applyHint')}</p>
        {/if}
      </div>
    {/if}
    {#if !narrow || mods.showDetail}
      <main class="mdet" aria-label={t('mods.details')}>
        {#if narrow}<button class="backlink" onclick={() => (mods.showDetail = false)}><Icon name="chev-l" size={16} />{t('mods.tab.mine')}</button>{/if}
        {#if cur}
          <ModDetail m={cur} />
        {:else if !ov}
          <span class="skel" style="height:72px;width:72px;border-radius:18px"></span>
          <span class="skel" style="height:24px;width:40%"></span>
          <span class="skel" style="height:120px"></span>
        {/if}
      </main>
    {/if}
  </div>
{/if}

<style>
  .pad { padding: 20px 24px; }
  .path { font-size: 13px; overflow-wrap: anywhere; }
  .mbody { flex: 1; min-height: 0; display: flex; overflow: hidden; }
  .mlist { width: 400px; flex: none; border-right: 2px solid var(--line); display: flex; flex-direction: column; min-height: 0; background: var(--bg); }
  :global(.narrow) .mlist { width: 100%; border-right: 0; }
  .tools { padding: 12px 14px; display: flex; flex-direction: column; gap: 10px; border-bottom: 2px solid var(--line); }
  .srch { position: relative; }
  .srch > :global(.ico) { position: absolute; left: 12px; top: 12px; color: var(--text-3); pointer-events: none; }
  .input {
    min-height: 44px;
    padding: 0 14px 0 42px;
    border: 2px solid var(--line-strong);
    border-radius: 10px;
    background: var(--surface);
    color: var(--text);
    font: 15px var(--f-body);
    width: 100%;
  }
  .chips { display: flex; gap: 6px; flex-wrap: wrap; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 36px;
    padding: 0 12px;
    border-radius: 999px;
    border: 2px solid var(--line);
    background: var(--surface);
    color: var(--text-2);
    font: 700 13.5px var(--f-body);
    cursor: pointer;
    white-space: nowrap;
  }
  .chip[aria-pressed='true'] { background: var(--text); border-color: var(--text); color: var(--bg); }
  .n { font: 600 12px var(--f-mono); opacity: 0.8; }
  ul { list-style: none; margin: 0; padding: 6px; overflow: auto; flex: 1; min-height: 0; }
  .li { display: flex; gap: 12px; align-items: center; padding: 4px 10px 4px 4px; border-radius: 12px; border: 2px solid transparent; }
  .li[data-sel='true'] { background: var(--surface); border-color: var(--primary); box-shadow: 0 2px 0 color-mix(in oklab, var(--primary), transparent 60%); }
  .pick { flex: 1; min-width: 0; display: flex; gap: 12px; align-items: center; padding: 6px; border-radius: 10px; cursor: pointer; border: 0; background: none; color: inherit; font: inherit; text-align: start; }
  .li.att :global(.mico) { outline: 3px solid var(--warn); outline-offset: 2px; }
  .mm { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .nm { font-weight: 700; font-size: 15px; display: flex; gap: 6px; align-items: baseline; flex-wrap: wrap; }
  .v { font-weight: 400; }
  .bd { display: flex; gap: 4px; flex-wrap: wrap; }
  .none { padding: 16px; }
  .probs { padding: 8px 16px; border-top: 2px solid var(--line); }
  .probs ul { padding: 4px 0 0; overflow: visible; }
  .probs li { overflow-wrap: anywhere; }
  .foot { padding: 10px 16px; border-top: 2px solid var(--line); }
  .mdet { flex: 1; min-width: 0; overflow: auto; padding: 20px 24px 32px; display: flex; flex-direction: column; gap: 14px; }
  .mdet > :global(*) { flex: none; }
  .backlink {
    align-self: flex-start;
    display: inline-flex;
    gap: 4px;
    align-items: center;
    font: 700 14px var(--f-body);
    color: var(--primary-ink);
    background: none;
    border: 0;
    padding: 4px 0;
    cursor: pointer;
  }
  .skel { display: block; border-radius: 8px; background: linear-gradient(90deg, var(--surface-2), var(--surface-3), var(--surface-2)); background-size: 200% 100%; animation: sk 1.4s ease-in-out infinite; }
  @keyframes sk { to { background-position: -200% 0; } }
  @media (prefers-reduced-motion: reduce) { .skel { animation: none; } }
  .empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; gap: 12px; padding: 44px 24px; }
  .empty h2 { font-size: 26px; }
  .empty .muted { max-width: 520px; }
  .blk { display: flex; gap: 6px; margin-bottom: 6px; }
  .blk span { width: 44px; height: 44px; border-radius: 10px; box-shadow: 0 4px 0 rgba(0, 0, 0, 0.18); }
  .acts { display: flex; gap: 10px; flex-wrap: wrap; justify-content: center; }
</style>
