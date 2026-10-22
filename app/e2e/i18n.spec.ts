import { expect, test, type Page } from '@playwright/test';
import { en, tr, type Key } from '../src/lib/messages';
import { kk } from '../src/lib/messages-kk';
import { ru } from '../src/lib/messages-ru';
import { modsEn } from '../src/lib/messages-mods';
import { modsKk } from '../src/lib/messages-mods-kk';
import { modsRu } from '../src/lib/messages-mods-ru';
import { modsTr } from '../src/lib/messages-mods-tr';
import { mockTauri } from './mock';

/**
 * Every launcher language at the smallest window (1100 x 760, `minWidth`/`minHeight` in tauri.conf.json), light and
 * dark: screenshots into $SHOT_DIR/i18n (default test-results/shots/i18n; never committed) and a check that no text is
 * cut off or pushes the page sideways.
 */
const DIR = `${process.env.SHOT_DIR ?? 'test-results/shots'}/i18n`;
const DICTS: Record<'en' | 'tr' | 'ru' | 'kk', Record<Key, string>> = {
  en: { ...en, ...modsEn }, tr: { ...tr, ...modsTr }, ru: { ...ru, ...modsRu }, kk: { ...kk, ...modsKk }
};
type Lang = keyof typeof DICTS;
const MIN = { width: 1100, height: 760 };

/**
 * Elements whose text does not fit: wider than their box (clipped or overflowing) or sticking out of the window.
 * Search-result hints (`.rh`) are cut to one line with an ellipsis on purpose.
 */
async function overflows(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const out: string[] = [];
    const sel = 'button, a, h1, h2, h3, label, p, li, span, th, td, [role="status"], [role="radio"], option, .rt, .val';
    if (document.documentElement.scrollWidth > window.innerWidth + 1) out.push(`page scrolls sideways (${document.documentElement.scrollWidth}px)`);
    for (const el of document.querySelectorAll<HTMLElement>(sel)) {
      const r = el.getBoundingClientRect();
      if (!r.width || !r.height || !el.textContent?.trim() || el.closest('[aria-hidden="true"], .sr-only, pre, code, .rh')) continue;
      const cs = getComputedStyle(el);
      if (cs.visibility === 'hidden' || cs.position === 'absolute' && r.width <= 1) continue;
      const clipped = el.scrollWidth > el.clientWidth + 1 && el.clientWidth > 0 && cs.display !== 'inline';
      const outside = r.right > window.innerWidth + 1;
      if (clipped || outside) out.push(`${el.tagName.toLowerCase()}.${[...el.classList].join('.')} "${el.textContent.trim().slice(0, 60)}" (${el.scrollWidth}/${el.clientWidth})`);
    }
    // Placeholders do not count in scrollWidth: measure them.
    const ctx = document.createElement('canvas').getContext('2d')!;
    for (const el of document.querySelectorAll<HTMLInputElement>('input[placeholder]')) {
      const cs = getComputedStyle(el);
      ctx.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
      const room = el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      if (el.clientWidth && ctx.measureText(el.placeholder).width > room + 1) out.push(`placeholder "${el.placeholder}" (${Math.round(ctx.measureText(el.placeholder).width)}/${room})`);
    }
    return [...new Set(out)];
  });
}

for (const lang of Object.keys(DICTS) as Lang[]) {
  const T = (k: Key) => DICTS[lang][k];
  for (const scheme of ['light', 'dark'] as const) {
    test.describe(`${lang} ${scheme}`, () => {
      let problems: string[] = [];
      test.beforeEach(() => void (problems = []));
      const open = async (page: Page, query: string) => {
        await mockTauri(page);
        await page.setViewportSize(MIN);
        await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
        await page.goto(`/?lang=${lang}&${query}`);
        await page.evaluate(() => localStorage.setItem('bhpc.setup-seen', '1'));
        await expect(page.locator('html')).toHaveAttribute('lang', lang);
      };
      const check = async (page: Page, name: string) => {
        await page.waitForTimeout(100);
        await page.screenshot({ path: `${DIR}/${lang}_${name}_${scheme}.png` });
        problems.push(...(await overflows(page)).map((p) => `${name}: ${p}`));
      };
      test.afterEach(() => expect(problems, `text that does not fit in ${lang}`).toEqual([]));

      test('setup', async ({ page }) => {
        await open(page, 'first=1');
        await check(page, 'setup1');
        await page.getByRole('checkbox').first().check();
        await page.getByRole('button', { name: T('common.next') }).click();
        await page.getByRole('button', { name: T('common.browse') }).click();
        await expect(page.getByText('The Blockheads 1.7.6')).toBeVisible();
        await check(page, 'setup2');
        for (const n of [3, 4, 5]) {
          await page.getByRole('button', { name: T('common.next') }).click();
          await check(page, `setup${n}`);
        }
        await page.getByRole('button', { name: T('common.browse') }).click();
        await page.getByRole('button', { name: T('imp.import'), exact: true }).click();
        await check(page, 'setup5-done');
        await page.getByRole('button', { name: T('common.next') }).click();
        await check(page, 'setup6');
      });

      for (const g of ['ready', 'running', 'crashed', 'missing'] as const)
        test(`home ${g}`, async ({ page }) => {
          await open(page, `game=${g}&updates=1&update=available`);
          await expect(page.getByRole('heading', { name: /The Blockheads/ }).first()).toBeVisible();
          await check(page, `home-${g}`);
        });

      test('settings and dialogs', async ({ page }) => {
        await open(page, 'updates=1');
        await page.getByRole('button', { name: T('set.title'), exact: true }).click();
        const nav = page.getByRole('navigation', { name: T('set.navLabel') });
        for (const p of ['game', 'gfx', 'controls', 'audio', 'lang', 'about'] as const) {
          await nav.getByRole('button', { name: T(`set.nav.${p}`) }).click();
          await check(page, `settings-${p}`);
        }
        await page.getByRole('searchbox').fill('fps');
        await check(page, 'settings-search');
        await page.getByRole('button', { name: T('set.back') }).click();
        await page.getByRole('button', { name: T('home.q.logs') }).click();
        await check(page, 'dialog-logs');
        await page.keyboard.press('Escape');
        await page.getByRole('button', { name: T('home.q.join') }).click();
        await check(page, 'dialog-join');
        await page.keyboard.press('Escape');
        await page.getByRole('button', { name: new RegExp(T('home.update')) }).first().click();
        await check(page, 'dialog-update');
        await page.keyboard.press('Escape');
        await page.getByRole('button', { name: T('home.play'), exact: true }).click();
        await expect(page.getByText(T('play.title'))).toBeVisible();
        await check(page, 'playing');
      });

      test('mods', async ({ page }) => {
        await open(page, 'dev=1');
        await page.getByRole('button', { name: new RegExp(`^${T('mods.title')}`) }).click();
        await expect(page.getByRole('tab', { name: new RegExp(T('mods.tab.mine')) })).toBeVisible();
        await check(page, 'mods-mine');
        await page.getByRole('button', { name: /Pixel Shaders/ }).click();
        await check(page, 'mods-native');
        for (const tab of ['browse', 'servers', 'dev'] as const) {
          await page.getByRole('tab', { name: new RegExp(T(`mods.tab.${tab}`)) }).click();
          await page.waitForTimeout(150);
          await check(page, `mods-${tab}`);
        }
        await page.getByRole('tab', { name: new RegExp(T('mods.tab.browse')) }).click();
        await page.getByRole('button', { name: 'Turbo Lighting', exact: true }).click();
        await page.locator('.dhead').getByRole('button').click();
        await check(page, 'mods-install-perms');
        await page.getByRole('button', { name: T('mods.inst.turnOn') }).click();
        await check(page, 'mods-install-native');
        await page.getByRole('button', { name: T('mods.inst.dont') }).click();
        await page.getByRole('button', { name: T('tb.home') }).click();
        await page.getByRole('button', { name: T('set.title'), exact: true }).click();
        await page.getByRole('navigation', { name: T('set.navLabel') }).getByRole('button', { name: T('set.nav.mods') }).click();
        await check(page, 'settings-mods');
      });
    });
  }
}

test('a first start follows the system language', async ({ browser }) => {
  for (const [locale, heading] of [['ru-RU', ru['welcome.title']], ['kk-KZ', kk['welcome.title']], ['de-DE', en['welcome.title']]]) {
    const page = await browser.newPage({ locale });
    await mockTauri(page);
    await page.goto('/?first=1&fresh=1');
    await expect(page.getByRole('heading', { name: heading })).toBeVisible();
    await page.close();
  }
});

test('the picker switches to Russian and Kazakh', async ({ page }) => {
  await mockTauri(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'Language & theme' }).click();
  const picker = page.getByLabel('Launcher language');
  await expect(picker.locator('option')).toHaveText(['English', 'Türkçe', 'Русский', 'Қазақша']);
  await picker.selectOption('ru');
  await expect(page.getByRole('heading', { name: ru['set.lang'] })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', 'ru');
  await page.getByLabel(ru['set.language']).selectOption('kk');
  await expect(page.getByRole('heading', { name: kk['set.lang'] })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', 'kk');
  await expect.poll(() => page.evaluate(() => (window as any).__mock.prefs.language)).toBe('kk');
  // The search finds settings by their Kazakh titles.
  await page.getByRole('searchbox').fill('дыбыс');
  await expect(page.getByRole('region', { name: kk['set.search.results'] })).toContainText(kk['set.sound']);
});
