import { expect, test, type Page } from '@playwright/test';
import { mockTauri } from './mock';

const DIR = process.env.SHOT_DIR ?? 'test-results/shots';

/** Screenshots of every screen and state, light and dark, into $SHOT_DIR (default test-results/shots; never committed). */
const open = async (page: Page, query: string, scheme: 'light' | 'dark', size = { width: 1100, height: 760 }) => {
  await page.setViewportSize(size);
  await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
  await page.goto('/?' + query);
  await page.evaluate(() => localStorage.setItem('bhpc.setup-seen', '1'));
};
const shot = (page: Page, name: string, scheme: string) => page.screenshot({ path: `${DIR}/launcher_${name}_${scheme}.png` });

for (const scheme of ['light', 'dark'] as const) {
  test.describe(scheme, () => {
    test.beforeEach(async ({ page }) => mockTauri(page));

    test('splash window', async ({ page }) => {
      // The splash is its own 600x380 window; the main window stays empty (and hidden) while booting.
      const size = { width: 600, height: 380 };
      await open(page, 'x=1', scheme, size);
      await page.goto('/splash');
      await expect(page.getByRole('status', { name: 'Blockheads PC is starting' })).toBeVisible();
      await shot(page, 'splash-launcher', scheme);
      await page.goto('/splash?first=1&splash=slow');
      await expect(page.getByRole('status', { name: 'Blockheads PC setup is starting' })).toBeVisible();
      await shot(page, 'splash-setup', scheme);
      await page.goto('/splash?splash=error');
      await expect(page.getByRole('alert')).toBeVisible();
      await shot(page, 'splash-error', scheme);
    });

    test('setup flow', async ({ page }) => {
      await open(page, 'first=1', scheme);
      await expect(page.getByRole('heading', { name: 'Play The Blockheads on your computer' })).toBeVisible();
      await shot(page, 'setup1-welcome', scheme);
      await page.getByLabel('I own the game').check();
      await page.getByRole('button', { name: 'Next' }).click();

      await expect(page.getByRole('heading', { name: 'Choose your game file' })).toBeVisible();
      await shot(page, 'setup2-apk-idle', scheme);
      await page.getByRole('button', { name: 'Browse…' }).click();
      await expect(page.getByText('The Blockheads 1.7.6')).toBeVisible();
      await shot(page, 'setup2-apk-ok', scheme);
      await page.getByRole('button', { name: 'Next' }).click();

      await expect(page.getByRole('heading', { name: 'Window and graphics' })).toBeVisible();
      await shot(page, 'setup3-graphics', scheme);
      await page.getByRole('button', { name: 'Next' }).click();

      await expect(page.getByRole('heading', { name: 'Controls' })).toBeVisible();
      await page.getByRole('button', { name: 'Change binding for Pinch' }).click();
      await shot(page, 'setup4-controls-listening', scheme);
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Change binding for Back' }).click();
      await page.keyboard.press('Backspace');
      await expect(page.getByText('Backspace')).toBeVisible();
      await shot(page, 'setup4-controls', scheme);
      await page.getByRole('button', { name: 'Next' }).click();

      await expect(page.getByRole('heading', { name: 'Bring your worlds from your phone' })).toBeVisible();
      await shot(page, 'setup5-import-choose', scheme);
      await page.getByRole('button', { name: 'Browse…' }).click();
      await page.getByRole('button', { name: 'Import', exact: true }).click();
      await expect(page.getByText('14 files imported')).toBeVisible();
      await shot(page, 'setup5-import-done', scheme);
      await page.getByRole('button', { name: 'Next' }).click();

      await expect(page.getByRole('heading', { name: 'You’re ready to play!' })).toBeVisible();
      await shot(page, 'setup6-ready', scheme);
      await page.getByRole('button', { name: 'Start the Launcher' }).click();
      await expect(page.getByRole('status').filter({ hasText: 'Ready to play' })).toBeVisible();
    });

    for (const [name, file] of [
      ['unsupported', 'TheBlockheads-old.apk'],
      ['not-blockheads', 'some-other-game.apk'],
      ['damaged', 'damaged.apk'],
      ['unreadable', 'denied.apk']
    ] as const) {
      test(`apk error ${name}`, async ({ page }) => {
        await open(page, 'first=1', scheme);
        await page.getByLabel('I own the game').check();
        await page.getByRole('button', { name: 'Next' }).click();
        await page.evaluate((f) => ((window as any).__mock.pick = '/home/mira/Downloads/' + f), file);
        await page.getByRole('button', { name: 'Browse…' }).click();
        await expect(page.getByRole('alert')).toBeVisible();
        await expect(page.getByRole('button', { name: 'Next' })).toBeDisabled();
        await shot(page, `setup2-apk-${name}`, scheme);
        if (name === 'unsupported') {
          await page.getByRole('switch', { name: 'Allow unknown builds' }).click({ force: true });
          await expect(page.getByText('Unknown build · experimental')).toBeVisible();
          await shot(page, 'setup2-apk-unknown-allowed', scheme);
        }
      });
    }

    for (const g of ['ready', 'running', 'stopped', 'crashed', 'missing'] as const) {
      test(`home ${g}`, async ({ page }) => {
        await open(page, `game=${g}&updates=1`, scheme);
        await expect(page.getByRole('heading', { name: /The Blockheads/ })).toBeVisible();
        await shot(page, `home-${g}`, scheme);
        await page.setViewportSize({ width: 800, height: 600 });
        await shot(page, `home-small-${g}`, scheme);
      });
    }

    test('home starting, playing, dialogs', async ({ page }) => {
      await open(page, 'game=ready&updates=1', scheme);
      await page.getByRole('button', { name: 'Play', exact: true }).click();
      await expect(page.getByText('The game runs in its own window')).toBeVisible();
      await shot(page, 'playing', scheme);
      await page.getByRole('button', { name: 'Home' }).click();
      await shot(page, 'home-starting', scheme);
      await page.getByRole('button', { name: 'View logs' }).click();
      await shot(page, 'logs', scheme);
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Join a server' }).first().click();
      await shot(page, 'help-join', scheme);
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Import saves' }).click();
      await shot(page, 'import-dialog', scheme);
      await page.keyboard.press('Escape');
    });

    test('close while playing', async ({ page }) => {
      await open(page, 'game=running', scheme);
      await page.evaluate(() => (window as any).__bhpcClose?.());
      await page.getByRole('button', { name: 'Stop game' }).click();
      await expect(page.getByRole('status').filter({ hasText: 'Stopped' })).toBeVisible();
    });

    test('update dialog stages', async ({ page }) => {
      await open(page, 'updates=1', scheme);
      await page.getByRole('button', { name: /Update available/ }).click();
      await expect(page.getByRole('dialog', { name: /is here/ })).toBeVisible();
      await shot(page, 'update-available', scheme);
      await page.getByRole('button', { name: 'Download and install' }).click();
      await expect(page.getByRole('dialog', { name: /Downloading/ })).toBeVisible();
      await shot(page, 'update-downloading', scheme);
      await expect(page.getByRole('dialog', { name: 'Update installed' })).toBeVisible({ timeout: 8000 });
      await shot(page, 'update-ready', scheme);
    });

    test('settings', async ({ page }) => {
      await open(page, 'updates=1', scheme, { width: 1100, height: 900 });
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      const nav = page.getByRole('navigation', { name: 'Settings sections' });
      for (const [name, file] of [
        ['Game & data', 'game'],
        ['Window & graphics', 'graphics'],
        ['Audio', 'audio'],
        ['Language & theme', 'language'],
        ['Updates & about', 'about']
      ] as const) {
        await nav.getByRole('button', { name }).click();
        await shot(page, `settings-${file}`, scheme);
      }
      await nav.getByRole('button', { name: 'Controls' }).click();
      await shot(page, 'settings-controls', scheme);
      await page.getByRole('button', { name: 'Change binding for Back' }).click();
      await page.keyboard.press('F11');
      await expect(page.getByRole('alert')).toContainText('F11 is used twice');
      await shot(page, 'settings-controls-conflict', scheme);
      const box = page.getByRole('searchbox', { name: 'Search settings' });
      await box.fill('fps');
      await shot(page, 'settings-search', scheme);
      await box.fill('zzz');
      await shot(page, 'settings-search-empty', scheme);
      await box.fill('speed');
      await page.getByRole('region', { name: 'Search results' }).getByRole('button', { name: /^Game speed/ }).click();
      await page.waitForTimeout(150);
      await shot(page, 'settings-search-opened', scheme);
    });

    test('small window 800 x 600', async ({ page }) => {
      const small = { width: 800, height: 600 };
      await open(page, 'first=1', scheme, small);
      await page.getByLabel('I own the game').check();
      await shot(page, 'small-setup1-welcome', scheme);
      await page.getByRole('button', { name: 'Next' }).click();
      await page.getByRole('button', { name: 'Browse…' }).click();
      await expect(page.getByText('The Blockheads 1.7.6')).toBeVisible();
      await shot(page, 'small-setup2-apk', scheme);
      await page.getByRole('button', { name: 'Next' }).click();
      await page.getByRole('button', { name: 'Next' }).click();
      await shot(page, 'small-setup4-controls', scheme);
      await page.goto('/?updates=1');
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      await shot(page, 'small-settings-general', scheme);
      await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'Controls' }).click();
      await shot(page, 'small-settings-controls', scheme);
      await page.getByRole('searchbox', { name: 'Search settings' }).fill('sound');
      await shot(page, 'small-settings-search', scheme);
    });

    for (const os of ['windows', 'macos', 'linux'] as const) {
      test(`title bar ${os}`, async ({ page }) => {
        await open(page, `os=${os}&maximized=1`, scheme);
        await expect(page.getByRole('banner', { name: 'Window title bar' })).toBeVisible();
        await page.locator('header[aria-label="Window title bar"]').screenshot({ path: `${DIR}/launcher_titlebar-${os}_${scheme}.png` });
      });
    }
  });
}
