<script lang="ts">
  import { ICONS, type IconName } from '../icons';

  let { name, size = 20 }: { name: IconName; size?: number } = $props();
</script>

<svg class="ico" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" focusable="false">
  {@html ICONS[name]}
</svg>

<style>
  .ico {
    flex: none;
    stroke: currentColor;
    fill: none;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
</style>
