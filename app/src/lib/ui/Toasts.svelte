<script lang="ts">
  import Icon from './Icon.svelte';
  import { dismiss, toasts } from './toast.svelte';
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.items as it (it.id)}
    <div class={['toast', it.tone]} role={it.tone === 'bad' ? 'alert' : 'status'}>
      <span class="ic"><Icon name={it.tone === 'bad' ? 'alert' : 'check'} /></span>
      <span class="msg">{it.message}</span>
      {#if it.action}
        <button
          class="act"
          onclick={() => {
            it.action?.();
            dismiss(it.id);
          }}>{it.actionLabel}</button
        >
      {/if}
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: 24px;
    transform: translateX(-50%);
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(460px, calc(100vw - 32px));
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 12px 12px 12px 16px;
    border-radius: 14px;
    background: var(--text);
    color: var(--bg);
    box-shadow: var(--lift);
  }
  .ic { display: flex; }
  .ok .ic { color: var(--ok); }
  .bad .ic { color: var(--bad); }
  .msg { flex: 1; font-weight: 700; min-width: 0; overflow-wrap: anywhere; }
  .act {
    min-height: 40px;
    padding: 0 12px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--bg);
    font: 700 14px var(--f-body);
    cursor: pointer;
    text-decoration: underline;
  }
</style>
