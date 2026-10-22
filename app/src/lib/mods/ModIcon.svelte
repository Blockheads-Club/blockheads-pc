<script lang="ts" module>
  /** Block tile colours of the design (tone 0-7). */
  export const TONES = ['#FFD27A', '#F6C79C', '#B9D7F5', '#BFE8CF', '#F5B9A8', '#D9C7F0', '#FFE3A3', '#C9E3E0'];

  /** Two letters: the first letters of the first two words, else the first two letters. */
  export function initials(name: string): string {
    const words = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
    const s = words.length > 1 ? words[0][0] + words[1][0] : (words[0] ?? '?').slice(0, 2);
    return s.toUpperCase();
  }

  /** A stable tone for a mod id. */
  export function toneOf(id: string): number {
    let h = 0;
    for (const c of id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    return h % TONES.length;
  }
</script>

<script lang="ts">
  let { id, name, size = 'md' }: { id: string; name: string; size?: 'sm' | 'md' | 'lg' } = $props();
</script>

<span class={['mico', size]} style:background={TONES[toneOf(id)]} aria-hidden="true">{initials(name)}</span>

<style>
  .mico {
    width: 44px;
    height: 44px;
    border-radius: 11px;
    display: grid;
    place-items: center;
    font: 800 16px var(--f-display);
    color: #1f1a17;
    transform: rotate(-4deg);
    box-shadow: 0 3px 0 rgba(0, 0, 0, 0.2);
    flex: none;
    letter-spacing: -0.02em;
  }
  .lg { width: 72px; height: 72px; border-radius: 18px; font-size: 26px; box-shadow: 0 5px 0 rgba(0, 0, 0, 0.2); }
  .sm { width: 32px; height: 32px; border-radius: 8px; font-size: 12.5px; }
</style>
