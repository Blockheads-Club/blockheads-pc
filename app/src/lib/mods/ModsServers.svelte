<script lang="ts">
  import { bytes } from '../format';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Badge from '../ui/Badge.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import DangerBadge from './DangerBadge.svelte';
  import ModIcon from './ModIcon.svelte';
  import { ago, errorText, mods } from './mods.svelte';
  import Notice from './Notice.svelte';

  const c = $derived(mods.servers);
  let open = $state<Record<string, boolean>>({});
</script>

<main class="full" aria-label={t('mods.tab.servers')}>
  <div class="top">
    <p class="muted lead">{t('mods.srv.lead')}</p>
    {#if c?.servers.length}
      <Button variant="danger-quiet" icon="trash" onclick={() => mods.clearServer(null)}>{t('mods.srv.clearAll', { size: bytes(c.total) })}</Button>
    {/if}
  </div>

  {#if mods.serversError}
    <Notice tone="bad" icon="alert" role="alert" title={t('mods.srv.error')}>
      <p class="muted">{errorText(mods.serversError)}</p>
      {#snippet actions()}<Button size="sm" variant="primary" icon="refresh" onclick={() => mods.loadServers()}>{t('common.retry')}</Button>{/snippet}
    </Notice>
  {:else if !c}
    <p class="sr" role="status">{t('mods.loading')}</p>
    <span class="skel" style="height:76px"></span><span class="skel" style="height:76px"></span>
  {:else if !c.servers.length}
    <div class="card empty">
      <Icon name="server" />
      <h2>{t('mods.srv.empty')}</h2>
      <p class="muted">{t('mods.srv.emptyBody')}</p>
    </div>
  {:else}
    {#each c.servers as s, i (s.key)}
      <details class="card srv" open={open[s.key] ?? i === 0} ontoggle={(e) => (open[s.key] = (e.currentTarget as HTMLDetailsElement).open)}>
        <summary>
          <ModIcon id={s.key} name={s.name} />
          <span class="st">
            <span class="sn">{s.name}<span class="mono hint addr">{s.address}</span></span>
            <span class="hint">{t('mods.srv.summary', { n: s.mods.length, size: bytes(s.size), when: ago(s.last_played) })}</span>
          </span>
          <Button size="sm" icon="trash" label={t('mods.srv.clearName', { name: s.name })} onclick={(e) => (e.preventDefault(), mods.clearServer(s.key))}>{t('mods.srv.clear')}</Button>
          <span class="chev" aria-hidden="true"><Icon name="chev-d" /></span>
        </summary>
        <ul>
          {#each s.mods as m (m.sha256)}
            <li>
              <ModIcon id={m.id} name={m.name} size="sm" />
              <span class="mn">{m.name}<span class="mono hint v">{m.version}</span></span>
              {#if m.native}<DangerBadge />{/if}
              {#each m.shared_with as other (other)}<Badge>{t('mods.srv.alsoUsed', { name: other })}</Badge>{/each}
              <span class="mono hint">{bytes(m.size)}</span>
            </li>
          {/each}
        </ul>
        <p class="hint">{t('mods.srv.only')}</p>
      </details>
    {/each}
    <p class="hint">{t('mods.srv.again')} <button class="lnk" onclick={() => store.openSettings('mods')}>{t('mods.srv.limitLink')}</button></p>
  {/if}
</main>

<style>
  .full { flex: 1; min-width: 0; overflow: auto; padding: 20px 24px 32px; display: flex; flex-direction: column; gap: 16px; max-width: 900px; }
  .full > :global(*) { flex: none; }
  .top { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; }
  .lead { flex: 1 1 360px; }
  .card { background: var(--surface); border: 2px solid var(--line); border-radius: 16px; }
  .srv { padding: 14px 18px; }
  summary { display: flex; gap: 14px; align-items: center; cursor: pointer; list-style: none; min-height: 44px; }
  summary::-webkit-details-marker { display: none; }
  .st { flex: 1; min-width: 0; }
  .sn { display: block; font-weight: 700; font-size: 16px; }
  .addr, .v { font-weight: 400; margin-left: 0.3em; }
  .chev { display: grid; transition: transform var(--dur-fast); }
  details[open] .chev { transform: rotate(180deg); }
  ul { list-style: none; margin: 8px 0 0; padding: 0; }
  li { display: flex; gap: 10px; align-items: center; padding: 8px 0; border-top: 1px solid var(--line); flex-wrap: wrap; }
  .mn { flex: 1; min-width: 0; font-weight: 700; font-size: 14.5px; }
  .srv > .hint { margin-top: 6px; }
  .empty { display: flex; flex-direction: column; align-items: center; text-align: center; gap: 12px; padding: 44px 24px; }
  .empty h2 { font-size: 22px; }
  .lnk { background: none; border: 0; padding: 0; color: var(--primary-ink); font: inherit; text-decoration: underline; text-underline-offset: 3px; cursor: pointer; }
  .skel { display: block; border-radius: 16px; background: linear-gradient(90deg, var(--surface-2), var(--surface-3), var(--surface-2)); background-size: 200% 100%; animation: sk 1.4s ease-in-out infinite; }
  @keyframes sk { to { background-position: -200% 0; } }
  @media (prefers-reduced-motion: reduce) { .skel { animation: none; } .chev { transition: none; } }
</style>
