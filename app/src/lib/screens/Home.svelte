<script lang="ts">
  import { api } from '../api';
  import { duration } from '../format';
  import { t } from '../i18n.svelte';
  import { REPO_URL, store } from '../store.svelte';
  import { effectiveDark } from '../theme.svelte';
  import type { IconName } from '../icons';
  import { baseName } from '../util';
  import { viewport } from '../viewport.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import PlayButton from '../ui/PlayButton.svelte';
  import Progress from '../ui/Progress.svelte';
  import StatusPill from '../ui/StatusPill.svelte';

  const g = $derived(store.game);
  const state = $derived(g.state);
  const dark = $derived(effectiveDark());
  const compact = $derived(viewport.compact);
  const minutes = $derived(Math.floor((g.running_secs ?? 0) / 60));
  const tail = $derived(store.log.slice(-3));
  const exit = $derived(
    g.signal !== null ? t('home.exitSignal', { n: g.signal }) : g.exit_code !== null ? t('home.exitCode', { n: g.exit_code }) : ''
  );
  const upd = $derived(['available', 'downloading', 'installing', 'ready'].includes(store.update.stage) ? store.update.info : null);
  const version = $derived(store.version);
  const file = $derived(baseName(store.config?.apk ?? ''));

  const quick = $derived<{ icon: IconName; title: string; hint: string; short: string; run: () => void }[]>([
    { icon: 'file', title: t('home.q.file'), hint: `${version} · ${file}`, short: t('home.q.fileShort'), run: () => store.openSettings('game') },
    { icon: 'map', title: t('home.q.import'), hint: t('home.q.importHint'), short: t('home.q.importShort'), run: () => { store.resetImport(); store.dialog = 'import'; } },
    { icon: 'folder', title: t('home.q.data'), hint: t('home.q.dataHint'), short: t('home.q.dataShort'), run: () => store.openDataFolder() },
    { icon: 'terminal', title: t('home.q.logs'), hint: t('home.q.logsHint'), short: t('home.q.logsShort'), run: () => (store.dialog = 'logs') },
    { icon: 'server', title: t('home.q.join'), hint: t('home.q.joinHint'), short: t('home.q.joinShort'), run: () => (store.dialog = 'join') }
  ]);
</script>

<div class={['app', compact && 'compact']}>
  <header class="top">
    {#if upd}
      <button class="upd" aria-haspopup="dialog" onclick={() => (store.dialog = 'update')}>
        <span class="updot" aria-hidden="true"></span>{t('home.update')}{#if !compact}<span class="mono ver">{upd.version}</span>{/if}
      </button>
    {/if}
    <span class="sp"></span>
    {#if compact}
      <Button variant="ghost" icon={dark ? 'sun' : 'moon'} label={dark ? t('theme.toLight') : t('theme.toDark')} onclick={() => store.patchPrefs({ theme: dark ? 'light' : 'dark' })} />
      <Button variant="ghost" icon="help" label={t('home.q.join')} onclick={() => (store.dialog = 'join')} />
    {:else}
      <Button size="sm" variant="ghost" pressed={dark} icon={dark ? 'sun' : 'moon'} onclick={() => store.patchPrefs({ theme: dark ? 'light' : 'dark' })}>
        {dark ? t('theme.lightMode') : t('theme.darkMode')}
      </Button>
      <Button size="sm" variant="ghost" icon="help" onclick={() => (store.dialog = 'join')}>{t('home.joinTop')}</Button>
    {/if}
    <Button size={compact ? 'md' : 'sm'} icon="sliders" onclick={() => store.openSettings()}>{t('set.title')}</Button>
  </header>

  <div class="body">
    <main class="stage sunburst" aria-labelledby="gtitle" id="main" tabindex="-1">
      <img class="logo" src="/pc-icon.svg" alt="" width={compact ? 96 : 132} height={compact ? 96 : 132} />
      <div class="info">
        <StatusPill {state} {minutes} />
        <h1 id="gtitle">{t('home.game')} {#if version}<span class="mono v">{version}</span>{/if}</h1>

        {#if state === 'ready'}
          <PlayButton size={compact ? 'md' : 'lg'} onclick={() => store.play()}>{t('home.play')}</PlayButton>
        {:else if state === 'starting'}
          <PlayButton size={compact ? 'md' : 'lg'} disabled busy>{t('home.starting')}</PlayButton>
          <div class="prog"><Progress stage label={t('home.startingLabel')} value={g.progress ? Math.floor(g.progress.percent) : undefined} /><p class="sub">{t('home.startingHint')}</p></div>
        {:else if state === 'running'}
          {#if !compact}<p class="lead">{t('home.runningLead')}</p>{/if}
          <div class="row">
            <PlayButton size="md" onclick={() => store.play()}>{t('home.back')}</PlayButton>
            <button class="sbtn stop" onclick={() => store.stop()}><Icon name="stop" />{t('home.stop')}</button>
          </div>
          {#if !compact}<p class="note">{t('home.saveHint')}</p>{/if}
        {:else if state === 'stopped'}
          <PlayButton size={compact ? 'md' : 'lg'} onclick={() => store.play()}>{t('home.playAgain')}</PlayButton>
          {#if g.last_session_secs !== null}<p class="sub">{t('home.last', { d: duration(g.last_session_secs) })}</p>{/if}
        {:else if state === 'crashed'}
          <div class="alertbox" role="alert">
            <p class="at"><Icon name="alert" />{t('home.crashed')}</p>
            {#if !compact}<p class="sub2">{t('home.crashedBody')}</p>{/if}
            <pre class="log" aria-label={t('home.lastLines')}>{[...tail, exit && `exit     ${exit}`].filter(Boolean).join('\n')}</pre>
            <div class="row left">
              <button class="sbtn sec" onclick={() => store.copyLog()}><Icon name="copy" />{t('logs.copy')}</button>
              {#if !compact}<button class="sbtn sec" onclick={() => store.saveLog()}><Icon name="download" />{t('logs.save')}</button>
              <button class="sbtn sec" onclick={() => api.openUrl(`${REPO_URL}/issues/new`)}><Icon name="code" />{t('home.report')}</button>{/if}
            </div>
          </div>
          <PlayButton size="md" onclick={() => store.play()}>{t('home.playAgain')}</PlayButton>
        {:else}
          <div class="alertbox warn" role="alert">
            <p class="at wt"><Icon name="file" />{t('home.missing')}</p>
            <p class="sub2">{t('home.missingBody', { name: file })}</p>
            <div class="row left"><button class="sbtn sun" onclick={() => store.browseApk()}><Icon name="folder" />{t('home.chooseFile')}</button></div>
          </div>
          <PlayButton size={compact ? 'md' : 'lg'} disabled describedby="np">{t('home.play')}</PlayButton>
          <p id="np" class="note">{t('home.missingNote')}</p>
        {/if}
      </div>
    </main>

    {#if compact}
      <nav class="qrow" aria-label={t('home.quick')}>
        {#each quick as q (q.icon)}
          <button class="qac" onclick={q.run}><Icon name={q.icon} />{q.short}</button>
        {/each}
      </nav>
    {:else}
      <nav class="side" aria-label={t('home.quick')}>
        {#each quick as q (q.icon)}
          <button class="qa" onclick={q.run}>
            <span class="qi"><Icon name={q.icon} /></span>
            <span class="qt">{q.title}<small>{q.hint}</small></span>
          </button>
        {/each}
      </nav>
    {/if}
  </div>
  <p class="legal">{t('home.legal')} <button class="lk" onclick={() => store.openSettings('about')}>{t('home.legalLink')}</button></p>
</div>

<style>
  .app { min-height: 100%; display: flex; flex-direction: column; background: var(--bg); }
  .top { display: flex; align-items: center; gap: 10px; padding: 10px 16px; border-bottom: 2px solid var(--line); background: var(--surface); flex-wrap: wrap; }
  .sp { flex: 1; }
  .upd {
    display: inline-flex; align-items: center; gap: 8px; min-height: 40px; padding: 0 14px; border-radius: 999px;
    background: var(--primary-soft); color: var(--primary-ink);
    border: 2px solid color-mix(in oklab, var(--primary), transparent 55%);
    font: 700 14px var(--f-body); cursor: pointer;
  }
  .updot { width: 8px; height: 8px; border-radius: 50%; background: var(--primary); }
  .ver { font-weight: 600; }
  .body { flex: 1; display: flex; flex-wrap: wrap; gap: 20px; padding: 20px; min-height: 0; }
  .stage {
    flex: 999 1 480px; min-width: 0; border-radius: 24px; color: var(--stage-text); padding: 32px;
    display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 18px; text-align: center;
    border: 2px solid #000; outline: none;
  }
  .logo { transform: rotate(-4deg); filter: drop-shadow(0 8px 0 rgba(0, 0, 0, 0.35)); }
  .info { display: flex; flex-direction: column; gap: 14px; align-items: center; }
  h1 { font-size: 30px; color: #f5ede5; }
  .v { font-size: 24px; font-weight: 600; }
  .lead { color: #f5ede5; font-size: 17px; max-width: 460px; }
  .note { color: var(--stage-text-3); font-size: 13.5px; }
  .sub { color: var(--stage-text-2); font-size: 14.5px; }
  .prog { width: min(360px, 100%); display: flex; flex-direction: column; gap: 8px; }
  .row { display: flex; gap: 10px; flex-wrap: wrap; justify-content: center; align-items: center; }
  .row.left { justify-content: flex-start; }
  .sbtn {
    display: inline-flex; align-items: center; justify-content: center; gap: 8px; min-height: 48px; padding: 0 20px;
    border-radius: 12px; font: 700 16px var(--f-body); cursor: pointer;
  }
  .sbtn:focus-visible { outline: 3px solid #86b9ff; outline-offset: 3px; }
  .stop { background: #ff8f80; color: #1c0d06; border: 2px solid #ff8f80; box-shadow: 0 3px 0 #8f3a2e; }
  .sec { background: #2a231f; color: #f5ede5; border: 2px solid #54483f; box-shadow: 0 3px 0 #000; min-height: 44px; font-size: 15px; }
  .sun { background: #ffd27a; color: #1f1a17; border: 2px solid #1f1a17; box-shadow: 0 3px 0 #c98a2e; }
  .alertbox {
    background: #2a1a17; border: 2px solid #ff8f80; border-radius: 16px; padding: 16px 18px; text-align: left;
    display: flex; flex-direction: column; gap: 10px; max-width: 560px; width: 100%;
  }
  .alertbox.warn { border-color: #f2c14e; background: #2a2416; }
  .at { display: flex; gap: 10px; align-items: center; font: 750 19px var(--f-display); color: #ffd2cb; }
  .at :global(.ico) { color: #ff8f80; }
  .at.wt { color: #ffe3a6; }
  .at.wt :global(.ico) { color: #f2c14e; }
  .sub2 { color: #dccfc3; font-size: 14.5px; }
  .log {
    background: #0e0b09; color: #f5ede5; font: 12.5px/1.6 var(--f-mono); padding: 10px 12px; border-radius: 10px; margin: 0;
    white-space: pre-wrap; overflow-wrap: anywhere; max-height: 96px; overflow: auto;
  }
  .side { flex: 1 1 260px; max-width: 340px; display: flex; flex-direction: column; gap: 12px; }
  .qa, .qac {
    border: 2px solid var(--line); background: var(--surface); color: var(--text); cursor: pointer; box-shadow: var(--edge);
    font-family: var(--f-body);
  }
  .qa { display: flex; align-items: center; gap: 12px; min-height: 56px; padding: 0 14px; border-radius: 14px; font-weight: 700; font-size: 15px; text-align: left; }
  .qi { width: 36px; height: 36px; border-radius: 10px; display: grid; place-items: center; background: var(--surface-2); color: var(--primary-ink); flex: none; }
  .qt { min-width: 0; }
  .qt small { display: block; font-weight: 400; color: var(--text-3); font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .qa:hover, .qac:hover { background: var(--surface-2); }
  .qrow { flex: 1 1 100%; display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 8px; }
  .qac {
    display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px; min-height: 68px; padding: 6px;
    border-radius: 12px; font: 700 12.5px/1.25 var(--f-body); text-align: center;
  }
  .qac :global(.ico) { color: var(--primary-ink); }
  .legal { font-size: 13px; color: var(--text-3); text-align: center; padding: 2px 20px 14px; text-wrap: balance; }
  .lk { background: none; border: 0; padding: 0; font: 700 13px var(--f-body); color: var(--text-2); text-decoration: underline; cursor: pointer; }
  .compact .qrow { flex: 0 0 auto; }
  .compact .body { padding: 12px; gap: 8px; flex-direction: column; flex-wrap: nowrap; }
  .compact .stage { flex: 1 1 auto; flex-direction: row; flex-wrap: wrap; border-radius: 20px; padding: 20px 24px; gap: 20px; text-align: left; justify-content: center; }
  .compact .info { align-items: flex-start; flex: 1; min-width: 220px; gap: 10px; }
  .compact h1 { font-size: 24px; }
  .compact .v { font-size: 20px; }
  .compact .row { justify-content: flex-start; }
  .compact .top { padding: 8px 12px; gap: 8px; flex-wrap: nowrap; }
  .compact .legal { font-size: 12.5px; padding: 0 12px 8px; }
</style>
