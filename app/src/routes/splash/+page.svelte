<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { setLang } from '$lib/i18n.svelte';
  import Splash from '$lib/screens/Splash.svelte';
  import type { SplashState } from '$lib/types';

  /** Contents of the separate splash window (src-tauri opens it first); the hidden main window pushes the state. */
  let state = $state<SplashState | null>(null);
  const square = navigator.userAgent.includes('Mac');

  function apply(s: SplashState) {
    setLang(s.lang);
    state = s;
  }

  onMount(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void api.onSplashState(apply).then((u) => (gone ? u() : (off = u))).catch(() => {});
    void api.splashGet().then((s) => s && !state && apply(s)).catch(() => {});
    return () => {
      gone = true;
      off?.();
    };
  });
</script>

<svelte:head><title>Blockheads PC</title></svelte:head>

{#if state}
  {@const dir = state.config_dir}
  <Splash {state} {square} onretry={() => void api.splashRetry()} onopenlog={() => void api.openPath(dir)} onquit={() => void api.appQuit()} />
{/if}

<style>
  :global(html, body) { background: transparent; overflow: hidden; }
</style>
