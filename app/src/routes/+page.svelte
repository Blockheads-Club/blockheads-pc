<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { api, win } from '$lib/api';
  import { i18n, t } from '$lib/i18n.svelte';
  import TitleBar from '$lib/layout/TitleBar.svelte';
  import CloseDialog from '$lib/screens/CloseDialog.svelte';
  import Home from '$lib/screens/Home.svelte';
  import ImportDialog from '$lib/screens/ImportDialog.svelte';
  import JoinDialog from '$lib/screens/JoinDialog.svelte';
  import LogsDialog from '$lib/screens/LogsDialog.svelte';
  import Playing from '$lib/screens/Playing.svelte';
  import SettingsAbout from '$lib/screens/SettingsAbout.svelte';
  import SettingsControls from '$lib/screens/SettingsControls.svelte';
  import SettingsGeneral from '$lib/screens/SettingsGeneral.svelte';
  import SetupApk from '$lib/screens/SetupApk.svelte';
  import SetupControls from '$lib/screens/SetupControls.svelte';
  import SetupGraphics from '$lib/screens/SetupGraphics.svelte';
  import SetupImport from '$lib/screens/SetupImport.svelte';
  import SetupReady from '$lib/screens/SetupReady.svelte';
  import SetupWelcome from '$lib/screens/SetupWelcome.svelte';
  import UpdateDialog from '$lib/screens/UpdateDialog.svelte';
  import { SETUP_STEPS, store } from '$lib/store.svelte';
  import { initTheme } from '$lib/theme.svelte';
  import Toasts from '$lib/ui/Toasts.svelte';
  import { watchViewport } from '$lib/viewport.svelte';
  import type { Os } from '$lib/types';

  /** The OS the bar imitates before the backend has answered. */
  function guessOs(): Os {
    const ua = navigator.userAgent;
    return ua.includes('Windows') ? 'windows' : ua.includes('Mac') ? 'macos' : 'linux';
  }
  const os = $derived(store.info?.os ?? guessOs());
  const s = $derived(store.screen);
  const onStage = $derived(s === 'playing');
  const ctx = $derived.by(() => {
    if (s === 'setup') return t('tb.setup', { n: store.step, total: SETUP_STEPS });
    if (s === 'settings') return store.section === 'controls' ? t('tb.settingsControls') : store.section === 'about' ? t('tb.settingsAbout') : t('tb.settings');
    if (s === 'playing') return t('tb.playing', { v: store.version });
    if (store.dialog === 'join') return t('tb.help');
    return s === 'home' ? t('tb.home') : '';
  });
  $effect(() => {
    document.title = ctx ? `${t('app.title')} · ${ctx}` : t('app.title');
  });

  /** Screen readers: after a step or screen change, focus moves to the new page's main region. */
  let shown = '';
  $effect(() => {
    const key = `${s}/${store.step}/${store.section}`;
    if (shown && key !== shown && s !== 'splash') void tick().then(() => document.getElementById('main')?.focus({ preventScroll: true }));
    shown = key;
  });

  /** Maximized or fullscreen: the window fills the screen, so no rounded corners. */
  let square = $state(false);
  // The game view inside the window is a native square widget: no rounded corners while it is up.
  const round = $derived(os !== 'macos' && !square && !store.embedded);

  /** The splash window mirrors the boot state; once past it the main window replaces the splash. */
  $effect(() => {
    if (s !== 'splash') return;
    const state = {
      boot: store.boot, error: store.bootError, first_run: store.firstRun,
      version: store.info?.version ?? '', config_dir: store.info?.config_dir ?? '', lang: i18n.lang
    };
    void api.splashSet(state).catch(() => {});
  });
  /** Our title bar is up (not in fullscreen play): dialogs leave it free so the window stays movable. */
  const titleBar = $derived(!(s === 'playing' && store.embedded && store.fullscreen));
  $effect(() => {
    document.documentElement.classList.toggle('has-tb', titleBar);
  });
  /** The tray menu follows the UI language. */
  $effect(() => {
    void api.trayLabels(t('tray.show'), t('tray.quit')).catch(() => {});
  });
  let revealed = false;
  $effect(() => {
    if (s === 'splash' || revealed) return;
    revealed = true;
    void api.mainReady().catch(() => {});
  });

  onMount(() => {
    initTheme();
    let offRetry: (() => void) | undefined;
    void api.onSplashRetry(() => void store.init()).then((u) => (offRetry = u)).catch(() => {});
    const sync = () => void Promise.all([win.isMaximized(), win.isFullscreen()]).then(([m, f]) => {
      square = m || f;
      store.fullscreen = f;
    }).catch(() => {});
    sync();
    let offResize: (() => void) | undefined;
    void win.onResized(sync).then((u) => (offResize = u)).catch(() => {});
    const unwatch = watchViewport();
    let off: (() => void) | undefined;
    void api.onQuitRequested(() => store.requestQuit()).then((u) => (off = u)).catch(() => {});
    void store.init();
    return () => {
      unwatch();
      off?.();
      offResize?.();
      offRetry?.();
      store.stopPolling();
    };
  });

  /** Keyboard shortcuts of the design (Components > Window sizes & keys). */
  function keydown(e: KeyboardEvent) {
    if (e.defaultPrevented) return;
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    if (s === 'home' && e.key === 'Enter' && plain && !store.dialog) {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if ((tag === 'BODY' || tag === 'MAIN') && ['ready', 'stopped', 'crashed'].includes(store.state)) {
        e.preventDefault();
        void store.play();
      }
    } else if (e.key === 'F11' && store.embedded) {
      e.preventDefault();
      void api.embedFullscreen(!store.fullscreen);
    } else if ((e.ctrlKey || e.metaKey) && e.key === ',' && store.config) {
      e.preventDefault();
      store.openSettings();
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'l' && store.config) {
      e.preventDefault();
      store.dialog = store.dialog === 'logs' ? null : 'logs';
    } else if (e.key === 'F1' && store.config) {
      e.preventDefault();
      store.dialog = 'join';
    } else if (s === 'setup' && e.altKey && e.key === 'ArrowLeft') {
      e.preventDefault();
      store.back();
    } else if (s === 'setup' && e.altKey && e.key === 'ArrowRight') {
      e.preventDefault();
      store.next();
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<a class="skip" href="#main">{t('common.skipContent')}</a>
<div class={['frame', round && 'round']}>
<div class="win">
  {#if titleBar}
  <TitleBar {os} {ctx} {onStage} native={store.info?.native_controls ?? false} />
  {/if}
  <div class="content">
    {#if s === 'splash'}
      <!-- Hidden window: the separate splash window shows the boot (see /splash). -->
    {:else if s === 'setup'}
      {#if store.step === 1}<SetupWelcome />
      {:else if store.step === 2}<SetupApk />
      {:else if store.step === 3}<SetupGraphics />
      {:else if store.step === 4}<SetupControls />
      {:else if store.step === 5}<SetupImport />
      {:else}<SetupReady />{/if}
    {:else if s === 'home'}
      <Home />
    {:else if s === 'playing'}
      <Playing />
    {:else if store.section === 'controls'}
      <SettingsControls />
    {:else if store.section === 'about'}
      <SettingsAbout />
    {:else}
      <SettingsGeneral />
    {/if}
  </div>
</div>

{#if store.dialog === 'join'}<JoinDialog />{/if}
{#if store.dialog === 'update'}<UpdateDialog />{/if}
{#if store.dialog === 'logs'}<LogsDialog />{/if}
{#if store.dialog === 'import'}<ImportDialog />{/if}
{#if store.dialog === 'close'}<CloseDialog />{/if}
<Toasts />
</div>

<style>
  /* The native window is transparent on Windows and Linux; this frame draws the rounded window (design: 12px).
     clip-path also clips the fixed dialogs and their scrim to the corners. */
  .frame { position: relative; height: 100vh; height: 100dvh; }
  .frame.round { clip-path: inset(0 round 12px); }
  .frame.round::after {
    content: ''; position: fixed; inset: 0; border: 1px solid var(--line-strong); border-radius: 12px;
    pointer-events: none; z-index: 1000;
  }
  .win { height: 100%; display: flex; flex-direction: column; background: var(--bg); }
  .content { flex: 1; min-height: 0; overflow: auto; position: relative; }
</style>
