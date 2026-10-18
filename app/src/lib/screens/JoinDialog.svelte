<script lang="ts">
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { GUIDE_URL, store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Card from '../ui/Card.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import Icon from '../ui/Icon.svelte';

  const online = ['join.o1', 'join.o2', 'join.o3'] as const;
  const byIp = ['join.i1', 'join.i2', 'join.i3'] as const;
</script>

<Dialog title={t('join.title')} description={t('join.lead')} wide onclose={() => (store.dialog = null)}>
  <div class="cols">
    <Card class="way" label={t('join.online')}>
      <div class="wh"><span class="wi info"><Icon name="globe" /></span><h3>{t('join.online')}</h3></div>
      <ol>{#each online as k, i (k)}<li><span class="n">{i + 1}</span><span>{t(k)}</span></li>{/each}</ol>
      <p class="hint">{t('join.onlineHint')}</p>
    </Card>
    <Card tone="primary" class="way" label={t('join.ip')}>
      <div class="wh"><span class="wi dark"><Icon name="server" /></span><h3>{t('join.ip')}</h3></div>
      <ol>{#each byIp as k, i (k)}<li><span class="n">{i + 1}</span><span>{t(k)}</span></li>{/each}</ol>
      <div class="addr" role="group" aria-label={t('join.example')}>
        <span><small>{t('join.address')}</small><b>192.168.1.42</b></span>
        <span><small>{t('join.port')}</small><b>15151</b></span>
      </div>
      <p class="hint"><Icon name="monitor" size={16} />{t('join.local')} <span class="mono">127.0.0.1</span></p>
    </Card>
  </div>
  <div class="cant"><Icon name="info" /><p><strong>{t('join.cantTitle')}</strong> {t('join.cant')}</p></div>
  {#snippet footer()}
    <div class="links">
      <button class="lk" onclick={() => api.openUrl(GUIDE_URL)}>{t('join.guide')}<Icon name="ext" size={16} /></button>
    </div>
    <Button variant="primary" onclick={() => (store.dialog = null)}>{t('join.gotIt')}</Button>
  {/snippet}
</Dialog>

<style>
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(300px, 100%), 1fr)); gap: 16px; }
  :global(.way) { padding: 18px; display: flex; flex-direction: column; gap: 12px; }
  .wh { display: flex; gap: 12px; align-items: center; }
  h3 { font-size: 20px; }
  .wi { width: 44px; height: 44px; border-radius: 12px; display: grid; place-items: center; transform: rotate(-6deg); box-shadow: 0 3px 0 rgba(0, 0, 0, 0.18); flex: none; }
  .wi.info { background: var(--info-soft); color: var(--info); }
  .wi.dark { background: #1f1a17; color: #ffd27a; }
  ol { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 10px; font-size: 15px; }
  li { display: flex; gap: 10px; }
  .n { width: 26px; height: 26px; border-radius: 8px; display: grid; place-items: center; background: var(--sun); color: #1f1a17; font: 800 13px var(--f-display); flex: none; transform: rotate(-6deg); }
  .addr { display: flex; gap: 6px; flex-wrap: wrap; }
  .addr span { display: flex; flex-direction: column; padding: 8px 12px; border-radius: 10px; background: var(--surface-2); border: 2px solid var(--line); }
  .addr small { font-size: 12px; color: var(--text-3); font-weight: 700; }
  .addr b { font: 600 16px var(--f-mono); }
  .hint { display: flex; gap: 8px; align-items: flex-start; flex-wrap: wrap; }
  .cant { display: flex; gap: 12px; align-items: flex-start; padding: 12px 14px; border-radius: 12px; background: var(--warn-soft); font-size: 14.5px; }
  .cant :global(.ico) { color: var(--warn); margin-top: 1px; }
  .links { flex: 1; display: flex; gap: 16px; flex-wrap: wrap; }
  .lk { display: inline-flex; gap: 6px; align-items: center; min-height: 44px; background: none; border: 0; padding: 0; font: 700 14.5px var(--f-body); color: var(--primary-ink); text-decoration: underline; cursor: pointer; }
</style>
