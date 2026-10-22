<script lang="ts">
  import { t } from '../i18n.svelte';
  import Icon from '../ui/Icon.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import { permInfo, permKey } from './perms';

  let { perm }: { perm: string } = $props();
  const info = $derived(permInfo(perm));
</script>

{#if info.tier === 'native'}
  <DangerBadge />
{:else}
  <span class={['perm', info.tier === 1 && 'l1', info.tier === 2 && 'l2']}>
    <Icon name={info.icon} size={14} />{t(permKey(info, ''), { host: info.host ?? '', name: perm })}
  </span>
{/if}

<style>
  .perm {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px 2px 7px;
    border-radius: 7px;
    font: 700 12.5px/1.5 var(--f-body);
    white-space: nowrap;
    background: var(--surface-2);
    color: var(--text-2);
    border: 1.5px solid var(--line);
  }
  .l1 { background: var(--warn-soft); color: var(--warn); border-color: color-mix(in oklab, var(--warn), transparent 65%); }
  .l2 { background: var(--bad-soft); color: var(--bad); border-color: color-mix(in oklab, var(--bad), transparent 55%); }
</style>
