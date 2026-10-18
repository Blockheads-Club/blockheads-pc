<script lang="ts">
  import { tick } from 'svelte';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import type { ActionId } from '../types';
  import { conflicts, keyToBinding, mouseToBinding, wheelToBinding } from '../util';
  import BindingChips from '../ui/BindingChips.svelte';
  import Button from '../ui/Button.svelte';

  const ACTIONS: ActionId[] = ['touch', 'pan', 'zoom', 'pinch', 'back', 'fullscreen'];
  /** Show the orange "changed" dot and its legend (Settings), not in setup. Per-row reset shows in both. */
  let { marks = false }: { marks?: boolean } = $props();

  let listening = $state<ActionId | null>(null);
  const keys = $derived(store.cfg?.keys);
  const dups = $derived(conflicts(store.cfg!.keys));
  const clash = (a: ActionId) => {
    const pair = dups.find(([x, y]) => x === a || y === a);
    return pair ? (pair[0] === a ? pair[1] : pair[0]) : null;
  };
  const changed = (a: ActionId) =>
    !!keys && !!store.defaults && keys[a].toLowerCase() !== store.defaults.keys[a].toLowerCase();

  $effect(() => {
    if (store.focusAction) {
      listening = store.focusAction;
      store.focusAction = null;
    }
  });

  async function listen(a: ActionId) {
    listening = a;
    await tick();
  }

  async function commit(text: string | null) {
    const a = listening;
    if (!a || text === null) return;
    listening = null;
    await store.setBinding(a, text);
  }

  /** While listening, the next input becomes the binding; Esc cancels. */
  function onkey(e: KeyboardEvent) {
    if (!listening) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape' && !e.ctrlKey && !e.shiftKey && !e.altKey) listening = null;
    else void commit(keyToBinding(e));
  }
  function onmouse(e: MouseEvent) {
    if (!listening || (e.target as HTMLElement).closest('[data-cancel]')) return;
    e.preventDefault();
    e.stopPropagation();
    void commit(mouseToBinding(e));
  }
  function onwheel(e: WheelEvent) {
    if (!listening) return;
    e.preventDefault();
    void commit(wheelToBinding(e));
  }
  const names = (a: ActionId) => t(`act.${a}` as 'act.touch');
</script>

<svelte:window onkeydowncapture={onkey} onmousedowncapture={onmouse} onwheel={onwheel} />

{#if keys}
  <div class="table" role="table" aria-label={t('ctl.tableLabel')}>
    {#each ACTIONS as a (a)}
      {@const other = clash(a)}
      <div class={['br', listening === a && 'listen', other && 'conf']} role="row">
        <span class="a" role="rowheader">
          {names(a)}
          {#if marks && changed(a)}<span class="chg" role="img" aria-label={t('ctl.changed')}></span>{/if}
          {#if other}<span class="same">{t('ctl.sameAs', { other: names(other) })}</span>{/if}
          <small>{t(`act.${a}.desc` as 'act.touch.desc')}</small>
        </span>
        <span role="cell">
          {#if listening === a}
            <span class="cap" role="status" aria-live="assertive">
              <span class="cdot" aria-hidden="true"></span>
              <span>{t('ctl.listening')}<span class="cesc">{t('ctl.escCancels')}</span></span>
            </span>
          {:else}
            <BindingChips binding={keys[a]} conflict={!!other} />
          {/if}
        </span>
        <span role="cell" class="btns">
          {#if listening === a}
            <span data-cancel><Button size="sm" onclick={() => (listening = null)}>{t('common.cancel')}</Button></span>
          {:else}
            <Button size="sm" label={t('ctl.changeFor', { action: names(a) })} onclick={() => listen(a)}>{t('ctl.change')}</Button>
            {#if changed(a)}
              <Button size="sm" variant="ghost" icon="reset" label={t('ctl.resetOne', { action: names(a) })} onclick={() => store.resetBinding(a)}>{t('ctl.reset')}</Button>
            {/if}
          {/if}
        </span>
      </div>
    {/each}
  </div>
  {#if marks}
    <p class="hint legend"><span class="chg" aria-hidden="true"></span>{t('ctl.legend')}</p>
  {/if}
{/if}

<style>
  .table { background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); }
  .br {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.25fr) 136px;
    gap: 12px;
    align-items: center;
    padding: 8px 16px;
    border-top: 1px solid var(--line);
    min-height: 60px;
  }
  .br:first-child { border-top: 0; border-radius: 14px 14px 0 0; }
  .br:last-child { border-radius: 0 0 14px 14px; }
  .a { font-weight: 700; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .a small { display: block; width: 100%; font-weight: 400; font-size: 12.5px; color: var(--text-3); }
  .same { font: 700 12.5px var(--f-body); color: var(--bad); }
  .br.conf { background: var(--bad-soft); }
  .br.listen { background: var(--primary-soft); border-radius: 12px; outline: 3px solid var(--focus); outline-offset: -3px; }
  .btns { display: flex; gap: 4px; justify-content: flex-end; }
  .cap {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    width: 100%;
    min-height: 46px;
    padding: 6px 14px;
    border-radius: 10px;
    border: 2px dashed var(--primary);
    background: var(--surface);
    color: var(--primary-ink);
    font: 700 14px/1.25 var(--f-body);
    text-align: left;
  }
  .cdot { width: 10px; height: 10px; border-radius: 50%; background: var(--primary); flex: none; animation: blink 1.2s ease-in-out infinite; }
  .cesc { display: block; font: 400 12.5px/1.3 var(--f-body); color: var(--text-3); }
  .chg { width: 8px; height: 8px; border-radius: 50%; background: var(--primary); display: inline-block; flex: none; }
  .legend { display: flex; gap: 8px; align-items: center; }
  @keyframes blink { 50% { opacity: 0.3; } }
  @media (max-width: 720px) {
    .br { grid-template-columns: 1fr auto; }
    .br > :nth-child(2) { grid-column: 1 / -1; grid-row: 2; }
  }
</style>
