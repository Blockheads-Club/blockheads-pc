<script lang="ts">
  import { t } from '../i18n.svelte';
  import { bytes } from '../format';
  import { store } from '../store.svelte';
  import { baseName } from '../util';
  import Alert from '../ui/Alert.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';
  import Progress from '../ui/Progress.svelte';
  import Spinner from '../ui/Spinner.svelte';

  /** Draw the Import button here (dialog) instead of in the page footer (setup). */
  let { actions = false }: { actions?: boolean } = $props();
  const s = $derived(store.importSummary);
  const empty = $derived(!!s && s.written === 0 && s.skipped_existing === 0);
  const sources = $derived([
    { id: 'folder' as const, icon: 'folder' as const, title: t('imp.folder'), hint: t('imp.folderHint') },
    { id: 'zip' as const, icon: 'zip' as const, title: t('imp.zip'), hint: t('imp.zipHint') }
  ]);
  const COLORS = ['#FFD27A', '#B9D7F5', '#E8D5F2', '#CDEBD8'];
</script>

{#if store.importing}
  <Card label={t('imp.progress')}>
    <div class="busy" aria-busy="true">
      <p class="bt"><Spinner />{t('imp.importing')}</p>
      <Progress label={t('imp.progress')} />
      <p class="hint mono">{baseName(store.importPath)}</p>
    </div>
  </Card>
{:else if s}
  {#if empty}
    <Card tone="bad" label={t('imp.noneTitle')}>
      <div class="none" role="alert">
        <span class="wi warn"><Icon name="alert" /></span>
        <div class="nb">
          <p class="h">{t('imp.noneTitle')}</p>
          <p class="muted">{t('imp.noneBody', { name: baseName(store.importPath) })}</p>
          <div class="acts">
            <Button size="sm" variant="primary" icon="folder" onclick={() => store.resetImport()}>{t('imp.chooseAnother')}</Button>
          </div>
        </div>
      </div>
    </Card>
  {:else}
    <div class="done" role="status">
      <Icon name="check" />
      <p><strong>{t('imp.done', { n: s.written, skipped: s.skipped_existing })}</strong> {t('imp.doneHint')}</p>
    </div>
    <Card pad={false} label={t('imp.results')}>
      {#each s.items as item, i (item.name)}
        <div class="wr">
          <span class="wi" style:background={COLORS[i % COLORS.length]}><Icon name="map" /></span>
          <div class="grow">
            <p class="wn mono">{item.name}</p>
            <p class="hint">{t('imp.files', { n: item.files })} · {bytes(item.bytes)}</p>
          </div>
          <Badge tone="ok">{t('imp.imported')}</Badge>
        </div>
      {/each}
      {#if s.skipped_existing > 0}
        <div class="wr">
          <span class="wi plain"><Icon name="skip" /></span>
          <div class="grow"><p class="wn">{t('imp.skippedTitle', { n: s.skipped_existing })}</p><p class="hint">{t('imp.skippedHint')}</p></div>
          <Badge>{t('imp.skipped')}</Badge>
        </div>
      {/if}
      {#if s.ignored > 0}
        <div class="wr">
          <span class="wi plain"><Icon name="skip" /></span>
          <div class="grow"><p class="wn">{t('imp.ignoredTitle', { n: s.ignored })}</p><p class="hint">{t('imp.ignoredHint')}</p></div>
          <Badge>{t('imp.skipped')}</Badge>
        </div>
      {/if}
    </Card>
    <div><Button size="sm" icon="plus" onclick={() => store.resetImport()}>{t('imp.more')}</Button></div>
  {/if}
{:else}
  {#if store.importError}
    <Alert tone="bad" title={t('imp.failed')}>{store.importError}</Alert>
  {/if}
  <div class="src" role="radiogroup" aria-label={t('imp.from')}>
    {#each sources as src (src.id)}
      <label class={['card opt', store.importKind === src.id && 'on']}>
        <input type="radio" name="src" class="radio" checked={store.importKind === src.id} onchange={() => { store.importKind = src.id; store.importPath = ''; }} />
        <span class="grow">
          <span class="ot"><Icon name={src.icon} />{src.title}</span>
          <span class="hint blk">{src.hint}</span>
          {#if store.importKind === src.id}
            <span class="pick">
              <Button size="sm" onclick={() => store.browseImport()}>{t('common.browse')}</Button>
              <span class="mono path" data-testid="import-path">{store.importPath || t('imp.nothingChosen')}</span>
            </span>
          {/if}
        </span>
      </label>
    {/each}
  </div>
  <fieldset class="card ov">
    <legend>{t('imp.existing')}</legend>
    <label class="opt2">
      <input type="radio" name="ov" class="radio" checked={!store.importOverwrite} onchange={() => (store.importOverwrite = false)} />
      <span><strong>{t('imp.skip')}</strong><span class="hint blk">{t('imp.skipHint')}</span></span>
    </label>
    <label class="opt2">
      <input type="radio" name="ov" class="radio" checked={store.importOverwrite} onchange={() => (store.importOverwrite = true)} />
      <span><strong>{t('imp.replace')}</strong><span class="hint blk">{t('imp.replaceHint')}</span></span>
    </label>
  </fieldset>
  {#if actions}
    <div class="acts end"><Button variant="primary" disabled={!store.importPath} onclick={() => store.runImport()}>{t('imp.import')}</Button></div>
  {/if}
{/if}

<style>
  .busy { padding: 18px 20px; display: flex; flex-direction: column; gap: 12px; }
  .bt { font-weight: 700; display: flex; gap: 10px; align-items: center; }
  .done { display: flex; gap: 12px; align-items: center; padding: 14px 16px; border-radius: 14px; background: var(--ok-soft); }
  .done :global(.ico) { color: var(--ok); }
  .wr { display: flex; gap: 14px; align-items: center; padding: 12px 16px; border-top: 1px solid var(--line); }
  .wr:first-child { border-top: 0; }
  .wi { width: 40px; height: 40px; border-radius: 11px; display: grid; place-items: center; flex: none; color: #1f1a17; transform: rotate(-4deg); }
  .wi.plain { background: var(--surface-3); color: var(--text-2); transform: none; }
  .wi.warn { background: var(--warn-soft); color: var(--warn); transform: none; width: 48px; height: 48px; }
  .grow { flex: 1; min-width: 0; }
  .wn { font-weight: 700; overflow-wrap: anywhere; }
  .none { padding: 20px; display: flex; gap: 14px; align-items: flex-start; }
  .nb { flex: 1; display: flex; flex-direction: column; gap: 6px; }
  .h { font: 750 20px var(--f-display); }
  .acts { display: flex; gap: 10px; flex-wrap: wrap; margin-top: 4px; }
  .acts.end { justify-content: flex-end; }
  .src { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: var(--r-lg); }
  .opt { padding: 16px; display: flex; gap: 12px; align-items: flex-start; cursor: pointer; }
  .opt.on { border-color: var(--primary); box-shadow: 0 3px 0 color-mix(in oklab, var(--primary), transparent 60%); }
  .ot { display: flex; gap: 8px; align-items: center; font-weight: 700; }
  .blk { display: block; margin: 4px 0 10px; }
  .pick { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  .path { font-size: 13px; overflow-wrap: anywhere; min-width: 0; }
  .radio { width: 22px; height: 22px; accent-color: var(--primary); margin: 2px 0 0; flex: none; }
  .ov { padding: 14px 18px; display: flex; flex-direction: column; gap: 6px; margin: 0; }
  .ov legend { font-weight: 700; padding: 0 6px; font-size: 15px; }
  .opt2 { display: flex; gap: 12px; align-items: flex-start; min-height: 44px; padding-top: 4px; cursor: pointer; }
  .opt2 .blk { margin: 0; }
</style>
