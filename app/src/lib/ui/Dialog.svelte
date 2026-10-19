<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { t } from '../i18n.svelte';
  import type { IconName } from '../icons';
  import Button from './Button.svelte';
  import Icon from './Icon.svelte';

  interface Props {
    title: string;
    onclose: () => void;
    icon?: IconName;
    tone?: 'bad' | 'plain';
    description?: string;
    wide?: boolean;
    /** Stack above other dialogs. */
    alert?: boolean;
    children?: Snippet;
    footer?: Snippet;
  }
  let { title, onclose, icon, tone = 'plain', description, wide = false, alert = false, children, footer }: Props = $props();
  const uid = $props.id();
  let box: HTMLElement | undefined = $state();

  const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

  /** Move focus into the dialog, keep it there, and give it back to the opener on close. */
  $effect(() => {
    const opener = document.activeElement as HTMLElement | null;
    tick().then(() => (box?.querySelector<HTMLElement>('[data-autofocus]') ?? box?.querySelector<HTMLElement>(FOCUSABLE))?.focus());
    return () => opener?.focus?.();
  });

  function keydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key !== 'Tab' || !box) return;
    const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)];
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onkeydown={keydown} onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div class={['dlg', wide && 'wide']} role={alert ? 'alertdialog' : 'dialog'} aria-modal="true" aria-labelledby="{uid}-t" bind:this={box}>
    <header>
      {#if icon}<span class={['ic', tone]}><Icon name={icon} /></span>{/if}
      <div class="head">
        <h2 id="{uid}-t">{title}</h2>
        {#if description}<p class="muted">{description}</p>{/if}
      </div>
      <Button variant="ghost" size="sm" icon="x" label={t('common.close')} onclick={onclose} />
    </header>
    {#if children}<div class="body">{@render children()}</div>{/if}
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: var(--scrim);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
  }
  .dlg.wide { max-width: 820px; }
  .dlg {
    background: var(--surface);
    border: 2px solid var(--line-strong);
    border-radius: var(--r-xl);
    box-shadow: var(--lift);
    width: 100%;
    max-width: 560px;
    max-height: calc(100vh - 48px);
    overflow: auto;
    display: flex;
    flex-direction: column;
    animation: land var(--dur-move) var(--ease-block);
  }
  @keyframes land { from { transform: translateY(12px) scale(0.98); opacity: 0; } }
  header { display: flex; gap: 14px; align-items: flex-start; padding: 22px 16px 8px 24px; }
  .head { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 6px; padding-top: 2px; }
  h2 { font-size: 22px; }
  .ic { width: 44px; height: 44px; border-radius: 12px; display: grid; place-items: center; flex: none; }
  .ic.bad { background: var(--bad-soft); color: var(--bad); }
  .ic.plain { background: var(--primary-soft); color: var(--primary-ink); }
  .body { padding: 8px 24px 4px; display: flex; flex-direction: column; gap: 16px; }
  footer { display: flex; justify-content: flex-end; align-items: center; gap: 10px; padding: 16px 24px 22px; flex-wrap: wrap; }
  @media (max-width: 600px) {
    .scrim { align-items: flex-end; padding: 0; }
    .dlg { max-width: none; border-radius: var(--r-xl) var(--r-xl) 0 0; max-height: 92vh; }
  }
  /* Desktop: the title bar stays usable above the scrim (move, resize, close the window). */
  :global(html.has-tb) .scrim { top: var(--titlebar-h); }
  :global(html.has-tb) .dlg { max-height: calc(100vh - var(--titlebar-h) - 48px); }
</style>
