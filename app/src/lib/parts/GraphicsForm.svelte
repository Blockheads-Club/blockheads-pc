<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import { FPS_LIMITS, SIZES, recommendedSize } from '../util';
  import Row from '../ui/Row.svelte';
  import Segmented from '../ui/Segmented.svelte';
  import SelectField from '../ui/SelectField.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import type { FpsPosition, GameZoom } from '../types';
  import { FPS_POSITIONS, fpsPositionLabel } from '../fps-position';
  import { GAME_SPEEDS, gameSpeedLabel } from '../game-speed';

  const w = $derived(store.cfg?.window);
  const rec = $derived(recommendedSize(store.display).join('x'));
  const sizeKey = $derived(`${store.cfg!.window.width}x${store.cfg!.window.height}`);
  const screenKey = $derived(store.display ? `${store.display.width}x${store.display.height}` : '');
  const sizes = $derived.by(() => {
    const keys = SIZES.map(([a, b]) => `${a}x${b}`);
    for (const extra of [screenKey, sizeKey]) if (extra && !keys.includes(extra)) keys.push(extra);
    return keys.map((k) => {
      const size = k.replace('x', ' × ');
      // A size of no preset (the window was resized by hand, or launcher.toml says so).
      if (k === sizeKey && k !== screenKey && !SIZES.some(([a, b]) => `${a}x${b}` === k)) {
        return { value: k, label: t('gfx.custom', { size }) };
      }
      const tags = [k === rec && t('gfx.recommended'), k === screenKey && t('gfx.screen')].filter(Boolean);
      return { value: k, label: `${size}${tags.length ? ` (${tags.join(', ')})` : ''}` };
    });
  });
  const ZOOMS: { value: GameZoom; key: 'gfx.zoomPhone' | 'gfx.zoomClose' | 'gfx.zoomTablet' }[] = [
    { value: 'phone', key: 'gfx.zoomPhone' },
    { value: 'close', key: 'gfx.zoomClose' },
    { value: 'tablet', key: 'gfx.zoomTablet' }
  ];
  const scale = $derived(Math.round(store.cfg!.window.ui_scale * 100));
  /** Unlimited, the usual caps, and a hand-edited value from launcher.toml when it is none of those. */
  const fpsLimits = $derived.by(() => {
    const values = [...FPS_LIMITS];
    const cur = store.cfg!.window.fps_limit;
    if (!values.includes(cur)) values.push(cur);
    return values.map((n) => ({ value: String(n), label: n === 0 ? t('gfx.fpsUnlimited') : t('gfx.fpsN', { n }) }));
  });
  /** The offered speeds, and a hand-edited value from launcher.toml when it is none of those. */
  const speeds = $derived.by(() => {
    const values = [...GAME_SPEEDS];
    const cur = store.cfg!.window.game_speed;
    if (!values.includes(cur)) values.push(cur);
    return values.sort((a, b) => a - b).map((n) => ({ value: String(n), label: gameSpeedLabel(n) }));
  });
  const uid = $props.id();

  function size(v: string) {
    const [a, b] = v.split('x').map(Number);
    void store.setWindowSize(a, b);
  }
</script>

{#if w}
  <Row id="set-gfx-mode" title={t('gfx.mode')} titleId="{uid}-m" first>
    <Segmented
      label={t('gfx.mode')}
      value={w.fullscreen ? 'full' : 'window'}
      options={[
        { value: 'window', label: t('gfx.window'), icon: 'window' },
        { value: 'full', label: t('gfx.fullscreen'), icon: 'max' }
      ]}
      onchange={(v) => store.patchConfig((c) => (c.window.fullscreen = v === 'full'))}
    />
  </Row>
  <Row id="set-gfx-size" title={t('gfx.size')} hint={t('gfx.sizeHint')}>
    <SelectField label={t('gfx.size')} hideLabel value={sizeKey} options={sizes} onchange={size} />
  </Row>
  <Row id="set-gfx-zoom" title={t('gfx.zoom')} hint={t('gfx.zoomHint')}>
    <SelectField
      label={t('gfx.zoom')}
      hideLabel
      value={w.zoom}
      options={ZOOMS.map((z) => ({ value: z.value, label: t(z.key) }))}
      onchange={(v) => store.patchConfig((c) => (c.window.zoom = v as GameZoom))}
    />
  </Row>
  <Row id="set-gfx-vsync" title={t('gfx.vsync')} hint={t('gfx.vsyncHint')} titleId="{uid}-v">
    <Toggle label={t('gfx.vsync')} hideLabel checked={w.vsync} onchange={(v) => store.patchConfig((c) => (c.window.vsync = v))} />
  </Row>
  <Row id="set-gfx-fps-limit" title={t('gfx.fpsLimit')} hint={t('gfx.fpsLimitHint')}>
    <SelectField
      label={t('gfx.fpsLimit')}
      hideLabel
      value={String(w.fps_limit)}
      options={fpsLimits}
      onchange={(v) => store.patchConfig((c) => (c.window.fps_limit = Number(v)))}
    />
  </Row>
  <Row id="set-gfx-show-fps" title={t('gfx.showFps')} hint={t('gfx.showFpsHint')} titleId="{uid}-f">
    <Toggle label={t('gfx.showFps')} hideLabel checked={w.show_fps} onchange={(v) => store.patchConfig((c) => (c.window.show_fps = v))} />
  </Row>
  {#if w.show_fps}
    <Row id="set-gfx-fps-position" title={t('gfx.fpsPosition')} hint={t('gfx.fpsPositionHint')}>
      <SelectField
        label={t('gfx.fpsPosition')}
        hideLabel
        value={w.fps_position}
        options={FPS_POSITIONS.map((p) => ({ value: p, label: fpsPositionLabel(p) }))}
        onchange={(v) => store.patchConfig((c) => (c.window.fps_position = v as FpsPosition))}
      />
    </Row>
  {/if}
  <Row id="set-gfx-speed" title={t('gfx.gameSpeed')} hint={t('gfx.gameSpeedHint')}>
    <SelectField
      label={t('gfx.gameSpeed')}
      hideLabel
      value={String(w.game_speed)}
      options={speeds}
      onchange={(v) => store.patchConfig((c) => (c.window.game_speed = Number(v)))}
    />
  </Row>
  <Row id="set-gfx-parallel" title={t('gfx.parallel')} hint={t('gfx.parallelHint')} titleId="{uid}-p">
    <Toggle
      label={t('gfx.parallel')}
      hideLabel
      checked={w.parallel_threads}
      onchange={(v) => store.patchConfig((c) => (c.window.parallel_threads = v))}
    />
  </Row>
  <Row id="set-gfx-scale" title={t('gfx.scale')} hint={t('gfx.scaleHint')}>
    <input
      type="range"
      min="80"
      max="130"
      step="5"
      value={scale}
      aria-label={t('gfx.scale')}
      aria-valuetext={t('gfx.percent', { n: scale })}
      onchange={(e) => store.patchConfig((c) => (c.window.ui_scale = Number(e.currentTarget.value) / 100))}
    />
    <span class="mono val">{t('gfx.percent', { n: scale })}</span>
  </Row>
{/if}

<style>
  input[type='range'] { accent-color: var(--primary); width: 160px; height: 28px; }
  .val { font-weight: 600; width: 56px; text-align: right; }
</style>
