<script lang="ts">
  import { t } from '../i18n.svelte';
  import Icon from '../ui/Icon.svelte';

  /** `reviewed`: from the public list; `ext`: not from it; `update`: a newer version. */
  let { kind, version = '' }: { kind: 'reviewed' | 'ext' | 'update'; version?: string } = $props();
</script>

{#if kind === 'reviewed'}
  <span class="rev"><Icon name="shield-ok" size={16} />{t('mods.reviewed')}</span>
{:else if kind === 'ext'}
  <span class="ext"><Icon name="alert" size={14} />{t('mods.notListed')}</span>
{:else}
  <span class="upd"><Icon name="upd" size={14} />{t('mods.updateTo', { v: version })}</span>
{/if}

<style>
  .rev, .upd { display: inline-flex; align-items: center; gap: 5px; font: 700 12.5px var(--f-body); white-space: nowrap; }
  .rev { color: var(--ok); }
  .upd { color: var(--primary-ink); }
  .ext {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 9px 2px 7px;
    border-radius: 7px;
    font: 700 12.5px/1.5 var(--f-body);
    background: var(--warn-soft);
    color: var(--warn);
    white-space: nowrap;
    border: 1.5px dashed color-mix(in oklab, var(--warn), transparent 40%);
  }
</style>
