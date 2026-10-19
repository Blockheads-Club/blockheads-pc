<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import { conflicts } from '../util';
  import ControlsEditor from '../parts/ControlsEditor.svelte';
  import GamepadPanel from '../parts/GamepadPanel.svelte';
  import Alert from '../ui/Alert.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';

  const dups = $derived(store.cfg ? conflicts(store.cfg.keys) : []);
  const first = $derived(dups[0]);
  const dupKey = $derived(store.cfg!.keys[first[0]]);
  const name = (a: string) => t(`act.${a}` as 'act.touch');
  let confirm = $state(false);
</script>

<div class="head" id="set-ctl-reset">
  <h1>{t('ctl.title')}</h1>
  <Button size="sm" icon="reset" onclick={() => (confirm = true)}>{t('ctl.resetAllConfirm')}</Button>
</div>
{#if first}
  <Alert tone="bad" title={t('ctl.dupTitle', { key: dupKey })}>
    {t('ctl.dupBody', { a: name(first[0]), b: name(first[1]) })}
    {#snippet actions()}
      <Button size="sm" onclick={() => (store.focusAction = first[0])}>{t('ctl.changeNamed', { action: name(first[0]) })}</Button>
      {#if store.undoKeys}<Button size="sm" variant="ghost" onclick={() => store.undoBinding()}>{t('ctl.undo')}</Button>{/if}
    {/snippet}
  </Alert>
{/if}
<ControlsEditor marks />
<GamepadPanel />

{#if confirm}
  <Dialog title={t('ctl.resetTitle')} description={t('ctl.resetBody')} onclose={() => (confirm = false)}>
    {#snippet footer()}
      <Button onclick={() => (confirm = false)}>{t('common.cancel')}</Button>
      <Button variant="primary" onclick={async () => { await store.resetKeys(); confirm = false; }}>{t('ctl.resetDo')}</Button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .head { display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap; }
  h1 { font-size: 26px; }
</style>
