<script lang="ts">
  let {
    checked = $bindable(false),
    label,
    hideLabel = false,
    disabled = false,
    onchange
  }: { checked?: boolean; label: string; hideLabel?: boolean; disabled?: boolean; onchange?: (v: boolean) => void } = $props();
</script>

<label class="sw" class:disabled>
  <input type="checkbox" role="switch" bind:checked {disabled} onchange={() => onchange?.(checked)} />
  <span class="track" aria-hidden="true"></span>
  <span class:sr={hideLabel}>{label}</span>
</label>

<style>
  .sw { display: inline-flex; align-items: center; gap: 12px; position: relative; cursor: pointer; min-height: 44px; }
  .sw.disabled { cursor: not-allowed; opacity: 0.55; }
  input { position: absolute; opacity: 0; width: 46px; height: 28px; margin: 0; }
  .track {
    width: 46px;
    height: 28px;
    border-radius: 999px;
    background: var(--surface-3);
    border: 2px solid var(--line-strong);
    position: relative;
    flex: none;
    transition: background-color var(--dur-fast);
  }
  .track::after {
    content: '';
    position: absolute;
    top: 3px;
    left: 3px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--text-3);
    transition: transform var(--dur-press);
  }
  input:checked + .track { background: var(--primary); border-color: var(--primary-edge); }
  input:checked + .track::after { transform: translateX(18px); background: var(--on-primary); }
  input:focus-visible + .track { outline: 3px solid var(--focus); outline-offset: 2px; }
</style>
