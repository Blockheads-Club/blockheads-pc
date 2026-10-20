<script lang="ts">
  import { t } from '../i18n.svelte';
  import type { Key } from '../messages';
  import { store } from '../store.svelte';
  import SettingsGroup from '../layout/SettingsGroup.svelte';
  import Row from '../ui/Row.svelte';
  import Toggle from '../ui/Toggle.svelte';

  /** The fixed controller layout (`crates/bhpc-input/src/pad.rs`), in table order. */
  const PAD = ['leftStick', 'rightStick', 'dpad', 'a', 'b', 'x', 'y', 'lbrb', 'start', 'r3'] as const;
  const on = $derived(store.cfg?.controller ?? true);
  /** Older configurations take the default (off). */
  const tiles = $derived(store.cfg?.pad_tile_cursor ?? false);
</script>

{#if store.cfg}
  <SettingsGroup title={t('pad.title')}>
    <Row id="set-ctl-pad" first title={t('pad.enable')} hint={t('pad.enableHint')}>
      <Toggle label={t('pad.enable')} hideLabel checked={on} onchange={(v) => store.setController(v)} />
    </Row>
    {#if on}
      <Row id="set-ctl-pad-tiles" title={t('pad.tiles')} hint={t('pad.tilesHint')}>
        <Toggle label={t('pad.tiles')} hideLabel checked={tiles} onchange={(v) => store.setPadTileCursor(v)} />
      </Row>
      <div id="set-ctl-pad-map" class="map">
        <p class="t">{t('pad.mapTitle')}</p>
        <p class="hint">{t('pad.mapHint')}</p>
        <table aria-label={t('pad.tableLabel')}>
          <thead>
            <tr><th scope="col">{t('pad.colButton')}</th><th scope="col">{t('pad.colAction')}</th></tr>
          </thead>
          <tbody>
            {#each PAD as b (b)}
              <tr>
                <th scope="row"><kbd>{t(`pad.b.${b}` as Key)}</kbd></th>
                <td>{t(`pad.a.${b}` as Key)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </SettingsGroup>
{/if}

<style>
  .map { padding: 12px 0 16px; border-top: 1px solid var(--line); scroll-margin: 24px; }
  .t { font-weight: 700; }
  .hint { font-size: 12.5px; color: var(--text-3); margin-bottom: 10px; }
  table { width: 100%; border-collapse: separate; border-spacing: 0; border: 1px solid var(--line); border-radius: var(--r-md); }
  th, td { text-align: left; padding: 7px 12px; border-top: 1px solid var(--line); vertical-align: middle; }
  th[scope='row'] { width: 38%; font-weight: 400; }
  thead th { border-top: 0; font: 700 12.5px var(--f-body); color: var(--text-3); }
  kbd {
    font: 700 13px var(--f-body);
    padding: 2px 8px;
    border-radius: 6px;
    border: 1px solid var(--line-strong);
    background: var(--surface-2);
    white-space: nowrap;
  }
</style>
