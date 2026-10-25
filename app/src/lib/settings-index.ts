import { t } from './i18n.svelte';
import type { IconName } from './icons';
import type { Key } from './messages';
import type { ActionId } from './types';

/** One page per Settings sidebar item. */
export type SettingsPage = 'game' | 'gfx' | 'controls' | 'audio' | 'lang' | 'mods' | 'about';

export const PAGES: { id: SettingsPage; icon: IconName; labelKey: Key }[] = [
  { id: 'game', icon: 'file', labelKey: 'set.nav.game' },
  { id: 'gfx', icon: 'monitor', labelKey: 'set.nav.gfx' },
  { id: 'controls', icon: 'mouse', labelKey: 'set.nav.controls' },
  { id: 'audio', icon: 'sound', labelKey: 'set.nav.audio' },
  { id: 'lang', icon: 'globe', labelKey: 'set.nav.lang' },
  { id: 'mods', icon: 'puzzle', labelKey: 'set.nav.mods' },
  { id: 'about', icon: 'info', labelKey: 'set.nav.about' }
];

export const ACTIONS: ActionId[] = ['touch', 'pan', 'zoom', 'pinch', 'back', 'fullscreen', 'hide_ui', 'chat'];

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

const CONTROL_WORDS = [
  'key', 'keys', 'binding', 'mouse', 'button', 'tuş', 'fare', 'düğme', 'kontrol',
  'клавиша', 'клавиши', 'мышь', 'кнопка', 'управление', 'назначение', 'перне', 'тінтуір', 'батырма', 'басқару'
];
const ACTION_WORDS: Record<ActionId, string[]> = {
  touch: ['click', 'tap', 'drag', 'left click', 'tıkla', 'dokun', 'sürükle', 'нажатие', 'клик', 'перетащить', 'басу', 'шерту', 'сүйреу'],
  pan: ['move', 'camera', 'scroll', 'kaydır', 'kamera', 'görünüm', 'камера', 'прокрутка', 'сдвиг', 'жылжыту', 'көрініс'],
  zoom: ['wheel', 'scroll', 'zoom in', 'zoom out', 'tekerlek', 'yakınlaştır', 'uzaklaştır', 'колесо', 'масштаб', 'приблизить', 'отдалить', 'дөңгелек', 'жақындату', 'алыстату'],
  pinch: ['gesture', 'two finger', 'iki parmak', 'hareket', 'kıstır', 'жест', 'два пальца', 'щипок', 'қимыл', 'екі саусақ'],
  back: ['escape', 'esc', 'menu', 'geri', 'menü', 'kapat', 'назад', 'меню', 'закрыть', 'артқа', 'мәзір', 'жабу'],
  fullscreen: ['f11', 'full screen', 'tam ekran', 'полный экран', 'толық экран'],
  hide_ui: [
    'hud', 'hide', 'buttons', 'interface', 'screenshot', 'gizle', 'düğmeler', 'arayüz', 'ekran görüntüsü',
    'скрыть', 'кнопки', 'интерфейс', 'скриншот', 'жасыру', 'батырмалар'
  ],
  chat: ['chat', 'message', 'talk', 't', 'sohbet', 'mesaj', 'yaz', 'чат', 'сообщение', 'хабар', 'сөйлесу']
};
const MOD_WORDS = ['mod', 'mods', 'modlar', 'мод', 'моды', 'модтар', 'плагин', 'addon', 'eklenti'];
const PAD_WORDS = [
  'controller', 'gamepad', 'joystick', 'xbox', 'playstation', 'stick', 'kol', 'oyun kolu', 'gamepad',
  'геймпад', 'джойстик', 'контроллер', 'стик', 'таяқша'
];

export const SETTINGS: SettingEntry[] = [
  { id: 'set-game-file', page: 'game', titleKey: 'set.game', hintKey: 'set.h.gameFile', keywords: ['apk', 'file', 'version', 'game', 'oyun', 'dosya', 'sürüm', 'файл', 'версия', 'игра', 'нұсқа', 'ойын'] },
  { id: 'set-game-unknown', page: 'game', titleKey: 'apk.unknownTitle', hintKey: 'set.unknownHint', keywords: ['experimental', 'unsupported', 'force', 'deneysel', 'bilinmeyen', 'desteklenmeyen', 'экспериментальный', 'неизвестная', 'неподдерживаемая', 'тәжірибелік', 'белгісіз', 'қолдау көрсетілмейтін'] },
  { id: 'set-data-folder', page: 'game', titleKey: 'set.data', hintKey: 'set.dataLead', keywords: ['saves', 'worlds', 'folder', 'move', 'logs', 'kayıt', 'dünya', 'klasör', 'taşı', 'veri', 'günlük', 'сохранения', 'миры', 'папка', 'перенести', 'журнал', 'данные', 'сақтаулар', 'әлемдер', 'қалта', 'көшіру', 'деректер'] },

  { id: 'set-gfx-mode', page: 'gfx', titleKey: 'gfx.mode', hintKey: 'set.h.mode', keywords: ['fullscreen', 'full screen', 'window', 'windowed', 'tam ekran', 'pencere', 'görüntü', 'полный экран', 'окно', 'оконный', 'толық экран', 'терезе'] },
  { id: 'set-gfx-size', page: 'gfx', titleKey: 'gfx.size', hintKey: 'gfx.sizeHint', keywords: ['resolution', 'width', 'height', 'çözünürlük', 'boyut', 'genişlik', 'yükseklik', 'разрешение', 'размер', 'ширина', 'высота', 'ажыратымдылық', 'өлшем', 'ені', 'биіктігі'] },
  { id: 'set-gfx-vsync', page: 'gfx', titleKey: 'gfx.vsync', hintKey: 'gfx.vsyncHint', keywords: ['vsync', 'v-sync', 'tearing', 'sync', 'dikey senkronizasyon', 'yırtılma', 'вертикальная синхронизация', 'разрывы', 'синхронизация', 'тік синхрондау', 'синхрондау'] },
  { id: 'set-gfx-fps-limit', page: 'gfx', titleKey: 'gfx.fpsLimit', hintKey: 'gfx.fpsLimitHint', keywords: ['fps', 'frame rate', 'framerate', 'cap', 'kare hızı', 'sınır', 'limit', 'частота кадров', 'кадры', 'ограничение', 'лимит', 'кадр жиілігі', 'шектеу'] },
  { id: 'set-gfx-show-fps', page: 'gfx', titleKey: 'gfx.showFps', hintKey: 'gfx.showFpsHint', keywords: ['fps', 'counter', 'frame rate', 'overlay', 'sayaç', 'kare hızı', 'göster', 'счётчик', 'частота кадров', 'показать', 'санауыш', 'кадр жиілігі', 'көрсету'] },
  { id: 'set-gfx-fps-position', page: 'gfx', titleKey: 'gfx.fpsPosition', hintKey: 'gfx.fpsPositionHint', keywords: ['fps', 'corner', 'counter', 'köşe', 'konum', 'sayaç', 'угол', 'положение', 'счётчик', 'бұрыш', 'орны', 'санауыш'], fallback: 'set-gfx-show-fps' },
  { id: 'set-gfx-zoom', page: 'gfx', titleKey: 'gfx.zoom', hintKey: 'gfx.zoomHint', keywords: ['zoom', 'scale', 'size', 'big', 'small', 'phone', 'tablet', 'yakınlaştırma', 'ölçek', 'büyük', 'küçük', 'telefon', 'масштаб', 'приближение', 'крупно', 'телефон', 'планшет', 'жақындату', 'үлкен', 'кіші'] },
  { id: 'set-gfx-speed', page: 'gfx', titleKey: 'gfx.gameSpeed', hintKey: 'gfx.gameSpeedHint', keywords: ['speed', 'fast', 'slow', 'time', 'hız', 'hızlı', 'yavaş', 'zaman', 'скорость', 'быстро', 'медленно', 'время', 'жылдамдық', 'жылдам', 'баяу', 'уақыт'] },
  { id: 'set-gfx-parallel', page: 'gfx', titleKey: 'gfx.parallel', hintKey: 'gfx.parallelHint', keywords: ['thread', 'threads', 'core', 'cores', 'cpu', 'parallel', 'freeze', 'crash', 'çekirdek', 'iş parçacığı', 'paralel', 'işlemci', 'donma', 'çökme', 'потоки', 'ядра', 'процессор', 'параллельно', 'зависание', 'вылет', 'ағындар', 'ядро', 'қатар', 'қатып қалу', 'құлау'] },
  { id: 'set-gfx-scale', page: 'gfx', titleKey: 'gfx.scale', hintKey: 'gfx.scaleHint', keywords: ['ui', 'hud', 'size', 'zoom', 'bigger', 'smaller', 'fine-tune', 'world', 'arayüz', 'ölçek', 'büyük', 'küçük', 'ince ayar', 'dünya', 'yakınlaştırma', 'интерфейс', 'масштаб', 'размер', 'крупнее', 'мельче', 'точная настройка', 'мир', 'приближение', 'өлшем', 'үлкен', 'кіші', 'дәл баптау', 'әлем', 'жақындату'] },

  { id: 'set-ctl-reset', page: 'controls', titleKey: 'ctl.resetAllConfirm', hintKey: 'ctl.resetBody', keywords: ['reset', 'default', 'defaults', 'sıfırla', 'varsayılan', 'сбросить', 'сброс', 'по умолчанию', 'қалпына келтіру', 'әдепкі', ...CONTROL_WORDS] },
  ...ACTIONS.map((a): SettingEntry => ({
    id: `set-ctl-${a}`,
    page: 'controls',
    titleKey: `act.${a}` as Key,
    hintKey: `act.${a}.desc` as Key,
    keywords: [...ACTION_WORDS[a], ...CONTROL_WORDS]
  })),
  { id: 'set-ctl-dpad', page: 'controls', titleKey: 'osc.dpad', hintKey: 'osc.dpadHint', keywords: ['dpad', 'd-pad', 'arrows', 'arrow pad', 'joystick', 'on-screen', 'touch controls', 'yön tuşları', 'ok tuşları', 'ekran', 'dokunmatik', 'крестовина', 'стрелки', 'экранные', 'сенсорное', 'көрсеткілер', 'экрандағы', 'сенсорлық'] },
  { id: 'set-ctl-pad', page: 'controls', titleKey: 'pad.enable', hintKey: 'pad.enableHint', keywords: [...PAD_WORDS, 'on', 'off', 'aç', 'kapat', 'включить', 'выключить', 'қосу', 'өшіру'] },
  { id: 'set-ctl-pad-tiles', page: 'controls', titleKey: 'pad.tiles', hintKey: 'pad.tilesHint', keywords: [...PAD_WORDS, 'tile', 'tiles', 'grid', 'cursor', 'snap', 'step', 'block', 'kare', 'ızgara', 'imleç', 'blok', 'adım', 'клетка', 'сетка', 'курсор', 'блок', 'шаг', 'тор', 'меңзер', 'қадам'], fallback: 'set-ctl-pad' },
  { id: 'set-ctl-pad-map', page: 'controls', titleKey: 'pad.mapTitle', hintKey: 'pad.mapHint', keywords: [...PAD_WORDS, 'buttons', 'layout', 'düğmeler', 'düzen', 'кнопки', 'раскладка', 'батырмалар', 'орналасу'], fallback: 'set-ctl-pad' },

  { id: 'set-snd-sound', page: 'audio', titleKey: 'set.sound', hintKey: 'set.soundHint', keywords: ['sound', 'volume', 'music', 'effects', 'audio', 'ses', 'müzik', 'efekt', 'ses düzeyi', 'звук', 'громкость', 'музыка', 'эффекты', 'дыбыс', 'дыбыс деңгейі', 'әсерлер'] },
  { id: 'set-snd-mute', page: 'audio', titleKey: 'set.muteBg', hintKey: 'set.muteSoon', keywords: ['mute', 'background', 'silent', 'sessiz', 'arka plan', 'sustur', 'без звука', 'фон', 'тишина', 'отключить звук', 'дыбыссыз', 'өшіру'] },

  { id: 'set-lng-language', page: 'lang', titleKey: 'set.language', hintKey: 'set.languageHint', keywords: [
    'language', 'english', 'turkish', 'russian', 'kazakh', 'translation', 'dil', 'türkçe', 'ingilizce', 'rusça', 'kazakça', 'çeviri',
    'язык', 'русский', 'английский', 'турецкий', 'казахский', 'перевод', 'тіл', 'қазақша', 'орысша', 'ағылшынша', 'түрікше', 'аударма'
  ] },
  { id: 'set-lng-theme', page: 'lang', titleKey: 'set.theme', hintKey: 'set.h.theme', keywords: ['dark', 'light', 'mode', 'night', 'karanlık', 'koyu', 'açık', 'tema', 'gece', 'тёмная', 'светлая', 'тема', 'ночь', 'қараңғы', 'ашық', 'тақырып', 'түн'] },

  { id: 'set-mods-auto', page: 'mods', titleKey: 'mods.set.auto', hintKey: 'mods.set.autoHint', keywords: [...MOD_WORDS, 'update', 'automatic', 'güncelle', 'otomatik', 'обновление', 'автоматически', 'жаңарту', 'автоматты'] },
  { id: 'set-mods-servers', page: 'mods', titleKey: 'mods.set.fromServers', hintKey: 'mods.set.fromServersHint', keywords: [...MOD_WORDS, 'server', 'join', 'download', 'sunucu', 'katıl', 'indir', 'сервер', 'войти', 'скачать', 'сервер', 'кіру', 'жүктеу'] },
  { id: 'set-mods-cache', page: 'mods', titleKey: 'mods.set.storage', hintKey: 'mods.set.storageHint', keywords: [...MOD_WORDS, 'cache', 'storage', 'disk', 'space', 'limit', 'önbellek', 'depolama', 'alan', 'sınır', 'кэш', 'место', 'диск', 'лимит', 'кэш', 'орын', 'шектеу'] },
  { id: 'set-mods-clear', page: 'mods', titleKey: 'mods.set.clear', hintKey: 'mods.set.storageHint', keywords: [...MOD_WORDS, 'clear', 'delete', 'cache', 'temizle', 'sil', 'önbellek', 'очистить', 'удалить', 'кэш', 'тазалау', 'жою'] },
  { id: 'set-mods-native', page: 'mods', titleKey: 'mods.set.blockNative', hintKey: 'mods.set.blockNativeHint', keywords: [...MOD_WORDS, 'native', 'danger', 'safety', 'family', 'block', 'yerel', 'tehlike', 'güvenlik', 'aile', 'engelle', 'нативный', 'опасно', 'безопасность', 'семья', 'запретить', 'жергілікті', 'қауіп', 'қауіпсіздік', 'отбасы', 'тыйым'] },
  { id: 'set-mods-jvm', page: 'mods', titleKey: 'mods.set.jvmOn', hintKey: 'mods.set.jvmOnHint', keywords: [...MOD_WORDS, 'java', 'jvm', 'kotlin', 'jre', 'native', 'danger', 'tehlike', 'yerel', 'опасно', 'нативный', 'қауіп', 'жергілікті'] },
  { id: 'set-mods-java', page: 'mods', titleKey: 'mods.set.javaHome', hintKey: 'mods.set.javaHomeHint', keywords: [...MOD_WORDS, 'java', 'jvm', 'jre', 'java_home', 'folder', 'path', 'klasör', 'yol', 'папка', 'путь', 'қалта', 'жол'] },
  { id: 'set-mods-files', page: 'mods', titleKey: 'mods.set.files', hintKey: 'mods.set.filesHint', keywords: [...MOD_WORDS, 'file', 'link', 'zip', 'review', 'dosya', 'bağlantı', 'файл', 'ссылка', 'сілтеме'] },
  { id: 'set-mods-dev', page: 'mods', titleKey: 'mods.set.devMode', hintKey: 'mods.set.devModeHint', keywords: [...MOD_WORDS, 'developer', 'folder', 'reload', 'log', 'geliştirici', 'klasör', 'yeniden yükle', 'разработчик', 'папка', 'перезагрузка', 'әзірлеуші', 'қалта'] },
  { id: 'set-mods-folder', page: 'mods', titleKey: 'mods.set.folder', hintKey: 'mods.set.devModeHint', keywords: [...MOD_WORDS, 'folder', 'path', 'open', 'klasör', 'aç', 'папка', 'открыть', 'қалта', 'ашу'] },

  { id: 'set-upd-check', page: 'about', titleKey: 'upd.check', hintKey: 'set.h.updCheck', keywords: ['update', 'new version', 'upgrade', 'güncelle', 'güncelleme', 'yeni sürüm', 'обновление', 'обновить', 'новая версия', 'жаңарту', 'жаңа нұсқа'] },
  { id: 'set-upd-auto', page: 'about', titleKey: 'upd.auto', hintKey: 'upd.autoHint', keywords: ['automatic', 'weekly', 'update', 'otomatik', 'haftalık', 'güncelleme', 'автоматически', 'еженедельно', 'обновление', 'автоматты', 'апта сайын', 'жаңарту'] },
  { id: 'set-about-app', page: 'about', titleKey: 'about.title', hintKey: 'set.h.about', keywords: ['version', 'system info', 'github', 'source', 'copy', 'sürüm', 'sistem bilgisi', 'kaynak', 'kopyala', 'hakkında', 'версия', 'о системе', 'исходный код', 'копировать', 'о программе', 'нұсқа', 'жүйе туралы', 'бастапқы код', 'көшіру', 'бағдарлама туралы'] },
  { id: 'set-legal', page: 'about', titleKey: 'legal.title', hintKey: 'set.h.legal', keywords: ['legal', 'disclaimer', 'trademark', 'gpl', 'yasal', 'marka', 'правовая информация', 'товарный знак', 'отказ от ответственности', 'құқықтық', 'тауар белгісі'] },
  { id: 'set-licences', page: 'about', titleKey: 'lic.title', hintKey: 'lic.lead', keywords: ['licence', 'license', 'open source', 'notices', 'lisans', 'açık kaynak', 'лицензия', 'лицензии', 'открытый код', 'лицензиялар', 'ашық бастапқы код'] }
];

/**
 * Lower case without accents, and the Turkish dotless/dotted i folded to a plain i: "Işık" and "isik" match. The
 * same folding makes Russian ё match е ("счётчик", "счетчик") and й match и.
 */
export function fold(s: string): string {
  return s.normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase().replace(/ı/g, 'i');
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
