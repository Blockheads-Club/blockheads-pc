<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { IconName } from '../icons';
  import Icon from './Icon.svelte';

  interface Props {
    variant?: 'default' | 'primary' | 'danger' | 'ghost';
    size?: 'sm' | 'md' | 'lg';
    icon?: IconName;
    /** Accessible name; required for icon-only buttons. */
    label?: string;
    loading?: boolean;
    disabled?: boolean;
    type?: 'button' | 'submit';
    href?: string;
    pressed?: boolean;
    expanded?: boolean;
    haspopup?: boolean;
    block?: boolean;
    title?: string;
    onclick?: (e: MouseEvent) => void;
    children?: Snippet;
  }
  let {
    variant = 'default',
    size = 'md',
    icon,
    label,
    loading = false,
    disabled = false,
    type = 'button',
    href,
    pressed,
    expanded,
    haspopup,
    block = false,
    title,
    onclick,
    children
  }: Props = $props();
  const iconOnly = $derived(!children);
</script>

{#if href}
  <a class={["btn", variant, size, iconOnly && "icon-only", block && "block"]} {href} {title} aria-label={label} target="_blank" rel="noopener noreferrer">
    {#if icon}<Icon name={icon} size={size === 'sm' ? 18 : 20} />{/if}
    {@render children?.()}
  </a>
{:else}
  <button
    class={["btn", variant, size, iconOnly && "icon-only", block && "block"]}
    {type}
    {title}
    {onclick}
    disabled={disabled || loading}
    aria-label={label}
    aria-busy={loading || undefined}
    aria-pressed={pressed}
    aria-expanded={expanded}
    aria-haspopup={haspopup ? 'menu' : undefined}
  >
    {#if loading}<span class="spin" aria-hidden="true"></span>{:else if icon}<Icon name={icon} size={size === 'sm' ? 18 : 20} />{/if}
    {@render children?.()}
  </button>
{/if}

<style>
  .btn {
    --edge-color: var(--line-strong);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 44px;
    padding: 0 18px;
    border-radius: var(--r-md);
    border: 2px solid var(--line-strong);
    background: var(--surface);
    color: var(--text);
    font: 700 15px/1.2 var(--f-body);
    box-shadow: 0 3px 0 var(--edge-color);
    cursor: pointer;
    text-decoration: none;
    text-align: center;
    transition: transform var(--dur-press), box-shadow var(--dur-press), background-color var(--dur-fast);
  }
  .btn:hover:not(:disabled) { background: var(--surface-2); color: var(--text); }
  .btn:active:not(:disabled) { transform: translateY(2px); box-shadow: 0 1px 0 var(--edge-color); }
  .btn.sm { min-height: 40px; padding: 0 12px; font-size: 14px; }
  .btn.lg { min-height: 52px; padding: 0 24px; font-size: 16.5px; }
  .btn.icon-only { width: 44px; padding: 0; }
  .btn.icon-only.sm { width: 40px; }
  .btn.block { width: 100%; }
  .btn.primary {
    --edge-color: var(--primary-edge);
    background: var(--primary);
    border-color: var(--primary-edge);
    color: var(--on-primary);
  }
  .btn.primary:hover:not(:disabled) { background: var(--primary-hover); color: var(--on-primary); }
  .btn.danger {
    --edge-color: color-mix(in oklab, var(--bad), #000 40%);
    background: var(--bad);
    border-color: var(--bad);
    color: var(--on-bad);
  }
  .btn.danger:hover:not(:disabled) { background: var(--bad); color: var(--on-bad); filter: brightness(1.06); }
  .btn.ghost { background: transparent; border-color: transparent; box-shadow: none; color: var(--text-2); }
  .btn.ghost:hover:not(:disabled) { background: var(--surface-2); color: var(--text); }
  .btn.ghost:active:not(:disabled) { box-shadow: none; }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; box-shadow: none; }
  .spin {
    width: 20px;
    height: 20px;
    flex: none;
    border-radius: 50%;
    border: 3px solid color-mix(in oklab, currentColor, transparent 65%);
    border-top-color: currentColor;
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
