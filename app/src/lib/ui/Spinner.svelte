<script lang="ts">
  let { size = 22, light = false }: { size?: number; light?: boolean } = $props();
</script>

<span class={['spin', light && 'light']} style:width={`${size}px`} style:height={`${size}px`} aria-hidden="true"></span>

<style>
  .spin {
    flex: none;
    border-radius: 50%;
    border: 3px solid var(--surface-3);
    border-top-color: var(--primary);
    animation: spin 0.9s linear infinite;
  }
  .light { border-color: rgba(203, 190, 178, 0.3); border-top-color: #cbbeb2; }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
