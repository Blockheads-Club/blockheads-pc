<script lang="ts">
  interface Props {
    label: string;
    /** 0 to 100; omit for an indeterminate bar. */
    value?: number;
    valuetext?: string;
    /** Draw on the dark stage. */
    stage?: boolean;
  }
  let { label, value, valuetext, stage = false }: Props = $props();
</script>

<div
  class={['bar', stage && 'stage']}
  role="progressbar"
  aria-label={label}
  aria-valuemin={value === undefined ? undefined : 0}
  aria-valuemax={value === undefined ? undefined : 100}
  aria-valuenow={value}
  aria-valuetext={valuetext}
>
  {#if value === undefined}
    <span class="ind"></span>
  {:else}
    <span class="fill" style:width={`${value}%`}></span>
  {/if}
</div>

<style>
  .bar {
    height: 12px;
    border-radius: 999px;
    background: var(--surface-3);
    border: 2px solid var(--line-strong);
    overflow: hidden;
    position: relative;
  }
  .bar.stage { background: #3a312b; border-color: #54483f; }
  .fill { display: block; height: 100%; background: var(--primary); border-radius: 999px; transition: width var(--dur-move); }
  .stage .fill, .stage .ind { background: #ff7a45; }
  .ind {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 35%;
    border-radius: 999px;
    background: var(--primary);
    animation: slide 1.3s ease-in-out infinite;
  }
  @keyframes slide { from { left: -35%; } to { left: 100%; } }
</style>
