<script lang="ts">
  import Icon from './Icon.svelte';

  interface Props {
    label: string;
    value: string;
    options: { value: string; label: string; lang?: string }[];
    disabled?: boolean;
    hideLabel?: boolean;
    hint?: string;
    onchange: (v: string) => void;
  }
  let { label, value, options, disabled = false, hideLabel = false, hint, onchange }: Props = $props();
  const uid = $props.id();
</script>

<div class="field">
  <label class="lbl" class:sr={hideLabel} for={uid}>{label}</label>
  <div class="wrap">
    <select id={uid} class="inp" {disabled} {...{ value }} onchange={(e) => onchange(e.currentTarget.value)}>
      {#each options as o (o.value)}
        <option {...{ value: o.value }} lang={o.lang}>{o.label}</option>
      {/each}
    </select>
    <span class="chev"><Icon name="chev-d" /></span>
  </div>
  {#if hint}<p class="hint">{hint}</p>{/if}
</div>

<style>
  .field { min-width: 0; display: flex; flex-direction: column; gap: 6px; }
  .lbl { font-weight: 700; font-size: 14.5px; }
  .wrap { position: relative; }
  .inp {
    width: 100%;
    min-height: 44px;
    padding: 0 40px 0 14px;
    border: 2px solid var(--line-strong);
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--text);
    font: 16px/1.4 var(--f-body);
    appearance: none;
    min-width: 200px;
  }
  .chev { position: absolute; right: 12px; top: 12px; pointer-events: none; display: flex; }
  .inp:disabled { opacity: 0.55; }
</style>
