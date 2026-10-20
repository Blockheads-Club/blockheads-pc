import { t } from './i18n.svelte';
import type { IconName } from './icons';
import type { Key } from './messages';
import type { ActionId } from './types';

/** One page per Settings sidebar item. */
export type SettingsPage = 'game' | 'gfx' | 'controls' | 'audio' | 'lang' | 'about';

export const PAGES: { id: SettingsPage; icon: IconName; labelKey: Key }[] = [
  { id: 'game', icon: 'file', labelKey: 'set.nav.game' },
  { id: 'gfx', icon: 'monitor', labelKey: 'set.nav.gfx' },
  { id: 'controls', icon: 'mouse', labelKey: 'set.nav.controls' },
  { id: 'audio', icon: 'sound', labelKey: 'set.nav.audio' },
  { id: 'lang', icon: 'globe', labelKey: 'set.nav.lang' },
  { id: 'about', icon: 'info', labelKey: 'set.nav.about' }
];

export const ACTIONS: ActionId[] = ['touch', 'pan', 'zoom', 'pinch', 'back', 'fullscreen', 'hide_ui'];

/**
 * A searchable setting. `id` is also the DOM id of its row on the page (the scroll and focus target);
 * `fallback` is the row to show when the setting's own row is hidden (e.g. the counter position while
 * the counter is off). Keywords are extra words people may type, in every launcher language.
 */
export interface SettingEntry {
  id: string;
  page: SettingsPage;
  titleKey: Key;
  hintKey: Key;
  keywords: string[];
  fallback?: string;
}

const CONTROL_WORDS = ['key', 'keys', 'binding', 'mouse', 'button', 'tuş', 'fare', 'düğme', 'kontrol'];
const ACTION_WORDS: Record<ActionId, string[]> = {
  touch: ['click', 'tap', 'drag', 'left click', 'tıkla', 'dokun', 'sürükle'],
  pan: ['move', 'camera', 'scroll', 'kaydır', 'kamera', 'görünüm'],
  zoom: ['wheel', 'scroll', 'zoom in', 'zoom out', 'tekerlek', 'yakınlaştır', 'uzaklaştır'],
  pinch: ['gesture', 'two finger', 'iki parmak', 'hareket', 'kıstır'],
  back: ['escape', 'esc', 'menu', 'geri', 'menü', 'kapat'],
  fullscreen: ['f11', 'full screen', 'tam ekran'],
  hide_ui: ['hud', 'hide', 'buttons', 'interface', 'screenshot', 'gizle', 'düğmeler', 'arayüz', 'ekran görüntüsü']
};
const PAD_WORDS = ['controller', 'gamepad', 'joystick', 'xbox', 'playstation', 'stick', 'kol', 'oyun kolu', 'gamepad'];

export const SETTINGS: SettingEntry[] = [
  { id: 'set-game-file', page: 'game', titleKey: 'set.game', hintKey: 'set.h.gameFile', keywords: ['apk', 'file', 'version', 'game', 'oyun', 'dosya', 'sürüm'] },
  { id: 'set-game-unknown', page: 'game', titleKey: 'apk.unknownTitle', hintKey: 'set.unknownHint', keywords: ['experimental', 'unsupported', 'force', 'deneysel', 'bilinmeyen', 'desteklenmeyen'] },
  { id: 'set-data-folder', page: 'game', titleKey: 'set.data', hintKey: 'set.dataLead', keywords: ['saves', 'worlds', 'folder', 'move', 'logs', 'kayıt', 'dünya', 'klasör', 'taşı', 'veri', 'günlük'] },

  { id: 'set-gfx-mode', page: 'gfx', titleKey: 'gfx.mode', hintKey: 'set.h.mode', keywords: ['fullscreen', 'full screen', 'window', 'windowed', 'tam ekran', 'pencere', 'görüntü'] },
  { id: 'set-gfx-size', page: 'gfx', titleKey: 'gfx.size', hintKey: 'gfx.sizeHint', keywords: ['resolution', 'width', 'height', 'çözünürlük', 'boyut', 'genişlik', 'yükseklik'] },
  { id: 'set-gfx-vsync', page: 'gfx', titleKey: 'gfx.vsync', hintKey: 'gfx.vsyncHint', keywords: ['vsync', 'v-sync', 'tearing', 'sync', 'dikey senkronizasyon', 'yırtılma'] },
  { id: 'set-gfx-fps-limit', page: 'gfx', titleKey: 'gfx.fpsLimit', hintKey: 'gfx.fpsLimitHint', keywords: ['fps', 'frame rate', 'framerate', 'cap', 'kare hızı', 'sınır', 'limit'] },
  { id: 'set-gfx-show-fps', page: 'gfx', titleKey: 'gfx.showFps', hintKey: 'gfx.showFpsHint', keywords: ['fps', 'counter', 'frame rate', 'overlay', 'sayaç', 'kare hızı', 'göster'] },
  { id: 'set-gfx-fps-position', page: 'gfx', titleKey: 'gfx.fpsPosition', hintKey: 'gfx.fpsPositionHint', keywords: ['fps', 'corner', 'counter', 'köşe', 'konum', 'sayaç'], fallback: 'set-gfx-show-fps' },
  { id: 'set-gfx-speed', page: 'gfx', titleKey: 'gfx.gameSpeed', hintKey: 'gfx.gameSpeedHint', keywords: ['speed', 'fast', 'slow', 'time', 'hız', 'hızlı', 'yavaş', 'zaman'] },
  { id: 'set-gfx-parallel', page: 'gfx', titleKey: 'gfx.parallel', hintKey: 'gfx.parallelHint', keywords: ['thread', 'threads', 'core', 'cores', 'cpu', 'parallel', 'freeze', 'crash', 'çekirdek', 'iş parçacığı', 'paralel', 'işlemci', 'donma', 'çökme'] },
  { id: 'set-gfx-scale', page: 'gfx', titleKey: 'gfx.scale', hintKey: 'gfx.scaleHint', keywords: ['ui', 'hud', 'size', 'zoom', 'bigger', 'smaller', 'arayüz', 'ölçek', 'büyük', 'küçük'] },

  { id: 'set-ctl-reset', page: 'controls', titleKey: 'ctl.resetAllConfirm', hintKey: 'ctl.resetBody', keywords: ['reset', 'default', 'defaults', 'sıfırla', 'varsayılan', ...CONTROL_WORDS] },
  ...ACTIONS.map((a): SettingEntry => ({
    id: `set-ctl-${a}`,
    page: 'controls',
    titleKey: `act.${a}` as Key,
    hintKey: `act.${a}.desc` as Key,
    keywords: [...ACTION_WORDS[a], ...CONTROL_WORDS]
  })),
  { id: 'set-ctl-dpad', page: 'controls', titleKey: 'osc.dpad', hintKey: 'osc.dpadHint', keywords: ['dpad', 'd-pad', 'arrows', 'arrow pad', 'joystick', 'on-screen', 'touch controls', 'yön tuşları', 'ok tuşları', 'ekran', 'dokunmatik'] },
  { id: 'set-ctl-pad', page: 'controls', titleKey: 'pad.enable', hintKey: 'pad.enableHint', keywords: [...PAD_WORDS, 'on', 'off', 'aç', 'kapat'] },
  { id: 'set-ctl-pad-map', page: 'controls', titleKey: 'pad.mapTitle', hintKey: 'pad.mapHint', keywords: [...PAD_WORDS, 'buttons', 'layout', 'düğmeler', 'düzen'], fallback: 'set-ctl-pad' },

  { id: 'set-snd-sound', page: 'audio', titleKey: 'set.sound', hintKey: 'set.soundHint', keywords: ['sound', 'volume', 'music', 'effects', 'audio', 'ses', 'müzik', 'efekt', 'ses düzeyi'] },
  { id: 'set-snd-mute', page: 'audio', titleKey: 'set.muteBg', hintKey: 'set.muteSoon', keywords: ['mute', 'background', 'silent', 'sessiz', 'arka plan', 'sustur'] },

  { id: 'set-lng-language', page: 'lang', titleKey: 'set.language', hintKey: 'set.languageHint', keywords: ['language', 'english', 'turkish', 'translation', 'dil', 'türkçe', 'ingilizce', 'çeviri'] },
  { id: 'set-lng-theme', page: 'lang', titleKey: 'set.theme', hintKey: 'set.h.theme', keywords: ['dark', 'light', 'mode', 'night', 'karanlık', 'koyu', 'açık', 'tema', 'gece'] },

  { id: 'set-upd-check', page: 'about', titleKey: 'upd.check', hintKey: 'set.h.updCheck', keywords: ['update', 'new version', 'upgrade', 'güncelle', 'güncelleme', 'yeni sürüm'] },
  { id: 'set-upd-auto', page: 'about', titleKey: 'upd.auto', hintKey: 'upd.autoHint', keywords: ['automatic', 'weekly', 'update', 'otomatik', 'haftalık', 'güncelleme'] },
  { id: 'set-about-app', page: 'about', titleKey: 'about.title', hintKey: 'set.h.about', keywords: ['version', 'system info', 'github', 'source', 'copy', 'sürüm', 'sistem bilgisi', 'kaynak', 'kopyala', 'hakkında'] },
  { id: 'set-legal', page: 'about', titleKey: 'legal.title', hintKey: 'set.h.legal', keywords: ['legal', 'disclaimer', 'trademark', 'gpl', 'yasal', 'marka'] },
  { id: 'set-licences', page: 'about', titleKey: 'lic.title', hintKey: 'lic.lead', keywords: ['licence', 'license', 'open source', 'notices', 'lisans', 'açık kaynak'] }
];

/** Lower case without accents, and the Turkish dotless/dotted i folded to a plain i: "Işık" and "isik" match. */
export function fold(s: string): string {
  return s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/ı/g, 'i');
}

/**
 * The settings matching every word of `query` in their title, hint, page name or keywords. Settings whose title
 * holds every word come first; otherwise index order.
 */
export function searchSettings(query: string): SettingEntry[] {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const all = (text: string) => words.every((w) => text.includes(w));
  const hits = SETTINGS.filter((s) => {
    const page = PAGES.find((p) => p.id === s.page)!;
    return all(fold([t(s.titleKey), t(s.hintKey), t(page.labelKey), ...s.keywords].join(' ')));
  });
  const inTitle = hits.filter((s) => all(fold(t(s.titleKey))));
  return [...inTitle, ...hits.filter((s) => !inTitle.includes(s))];
}
