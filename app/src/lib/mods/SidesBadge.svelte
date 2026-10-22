<script lang="ts">
  import { t } from '../i18n.svelte';
  import Icon from '../ui/Icon.svelte';
  

  let { sides }: { sides: 'server' | 'client' | 'both' } = $props();
  const icon = { server: 'server', client: 'monitor', both: 'both' } as const;
</script>

<span class={['side', sides]}><Icon name={icon[sides]} size={14} />{t(`mods.sides.${sides}`)}</span>

<style>
  .side {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px 2px 7px;
    border-radius: 7px;
    font: 700 12.5px/1.5 var(--f-body);
    white-space: nowrap;
    border: 1.5px solid transparent;
  }
  .server { background: var(--ok-soft); color: var(--ok); }
  .client { background: var(--info-soft); color: var(--info); }
  .both { background: var(--primary-soft); color: var(--primary-ink); }
</style>
