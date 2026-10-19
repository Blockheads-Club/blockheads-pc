<script lang="ts">
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import { FPS_LIMITS, SIZES, recommendedSize } from '../util';
  import Row from '../ui/Row.svelte';
  import Segmented from '../ui/Segmented.svelte';
  import SelectField from '../ui/SelectField.svelte';
  import Toggle from '../ui/Toggle.svelte';

  const w = $derived(store.cfg?.window);
  const rec = $derived(recommendedSize(store.display).join('x'));
  const sizeKey = $derived(`${store.cfg!.window.width}x${store.cfg!.window.height}`);
  const screenKey = $derived(store.display ? `${store.display.width}x${store.display.height}` : '');
  const sizes = $derived.by(() => {
    const keys = SIZES.map(([a, b]) => `${a}x${b}`);
    for (const extra of [screenKey, sizeKey]) if (extra && !keys.includes(extra)) keys.push(extra);
    return keys.map((k) => {
      const tags = [k === rec && t('gfx.recommended'), k === screenKey && t('gfx.screen')].filter(Boolean);
      return { value: k, label: `${k.replace('x', ' × ')}${tags.length ? ` (${tags.join(', ')})` : ''}` };
    });
  });
  const scale = $derived(Math.round(store.cfg!.window.ui_scale * 100));
  /** Unlimited, the usual caps, and a hand-edited value from launcher.toml when it is none of those. */
  const fpsLimits = $derived.by(() => {
    const values = [...FPS_LIMITS];
    const cur = store.cfg!.window.fps_limit;
    if (!values.includes(cur)) values.push(cur);
    return values.map((n) => ({ value: String(n), label: n === 0 ? t('gfx.fpsUnlimited') : t('gfx.fpsN', { n }) }));
  });
  const uid = $props.id();

  function size(v: string) {
    const [a, b] = v.split('x').map(Number);
    void store.patchConfig((c) => {
      c.window.width = a;
      c.window.height = b;
    });
  }
</script>

{#if w}
  <Row title={t('gfx.mode')} titleId="{uid}-m" first>
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
  <Row title={t('gfx.size')} hint={t('gfx.sizeHint')}>
    <SelectField label={t('gfx.size')} hideLabel value={sizeKey} options={sizes} onchange={size} />
  </Row>
  <Row title={t('gfx.vsync')} hint={t('gfx.vsyncHint')} titleId="{uid}-v">
    <Toggle label={t('gfx.vsync')} hideLabel checked={w.vsync} onchange={(v) => store.patchConfig((c) => (c.window.vsync = v))} />
  </Row>
  <Row title={t('gfx.fpsLimit')} hint={t('gfx.fpsLimitHint')}>
    <SelectField
      label={t('gfx.fpsLimit')}
      hideLabel
      value={String(w.fps_limit)}
      options={fpsLimits}
      onchange={(v) => store.patchConfig((c) => (c.window.fps_limit = Number(v)))}
    />
  </Row>
  <Row title={t('gfx.showFps')} hint={t('gfx.showFpsHint')} titleId="{uid}-f">
    <Toggle label={t('gfx.showFps')} hideLabel checked={w.show_fps} onchange={(v) => store.patchConfig((c) => (c.window.show_fps = v))} />
  </Row>
  <Row title={t('gfx.scale')} hint={t('gfx.scaleHint')}>
    <input
      type="range"
      min="75"
      max="200"
      step="25"
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
