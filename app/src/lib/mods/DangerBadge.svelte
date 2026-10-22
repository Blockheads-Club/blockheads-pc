<script lang="ts">
  import { t } from '../i18n.svelte';
  import Icon from '../ui/Icon.svelte';

  /** `short`: just "Native code" (dialog headers); `word`: an inline "Danger" tag. */
  let { variant = 'full' }: { variant?: 'full' | 'short' | 'word' } = $props();
  const text = $derived(variant === 'full' ? t('mods.native') : variant === 'short' ? t('mods.nativeShort') : t('mods.danger'));
</script>

<span class="native" role="img" aria-label={t('mods.nativeAria')}><Icon name="octagon" size={14} /><span aria-hidden="true">{text}</span></span>

<style>
  /* Danger never by colour alone: stripes, octagon icon, uppercase word and a thick border. */
  .native {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px 2px 7px;
    border-radius: 7px;
    font: 800 12px/1.6 var(--f-body);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    /* Long translations wrap inside the badge rather than overflow narrow columns. */
    white-space: normal;
    max-width: 100%;
    color: var(--mod-on-danger);
    border: 2px solid var(--mod-danger-edge);
    background: repeating-linear-gradient(-45deg, var(--mod-danger) 0 8px, var(--mod-danger-stripe) 8px 16px);
    align-self: flex-start;
  }
  .native :global(.ico) { stroke-width: 2.5; }
</style>
