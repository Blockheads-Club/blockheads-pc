import { expect, test, type Page } from '@playwright/test';
import { mockTauri } from './mock';

const DIR = process.env.SHOT_DIR ?? 'test-results/shots';

const open = async (page: Page, query: string, scheme: 'light' | 'dark' = 'light', size = { width: 1100, height: 800 }) => {
  await page.setViewportSize(size);
  await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
  await page.goto('/?' + query);
  await page.evaluate(() => localStorage.setItem('bhpc.setup-seen', '1'));
};
const shot = (page: Page, name: string, scheme: string) => page.screenshot({ path: `${DIR}/mods_${name}_${scheme}.png` });

test.beforeEach(async ({ page }) => mockTauri(page));

test('Home shows the Mods button with the problem count and opens the Mods area', async ({ page }) => {
  await open(page, 'x=1');
  const btn = page.getByRole('button', { name: 'Mods, 2 problems' });
  await expect(btn).toBeVisible();
  await btn.click();
  await expect(page.getByRole('tab', { name: /My mods/ })).toHaveAttribute('aria-selected', 'true');
  // Keyboard: arrows switch tabs, Esc goes home.
  await page.getByRole('tab', { name: /My mods/ }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'Mod list' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('heading', { name: 'Sky Core' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Mods, 2 problems' })).toBeVisible();
});

test('installing a native mod starts on "Don’t install" and needs both ticks', async ({ page }) => {
  await open(page, 'x=1');
  await page.getByRole('button', { name: /^Mods/ }).click();
  await page.getByRole('tab', { name: 'Mod list' }).click();
  await page.getByRole('button', { name: 'Install Turbo Lighting' }).click();
  await page.getByRole('button', { name: 'Install and turn on' }).click();
  const dlg = page.getByRole('dialog');
  await expect(dlg.getByRole('button', { name: 'Don’t install' })).toBeFocused();
  const allow = dlg.getByRole('button', { name: 'Allow full access and install' });
  await expect(allow).toBeDisabled();
  await dlg.getByLabel('I understand what native code can do.').check();
  await expect(allow).toBeDisabled();
  await dlg.getByLabel(/I trust lumen/).check();
  await allow.click();
  await expect(dlg.getByRole('heading', { name: 'Turbo Lighting is ready' })).toBeVisible();
});

for (const scheme of ['light', 'dark'] as const) {
  test.describe(scheme, () => {
    test('mods screens', async ({ page }) => {
      test.setTimeout(120_000);
      await open(page, 'x=1', scheme);
      await shot(page, 'home', scheme);
      await page.getByRole('button', { name: /^Mods/ }).click();
      await expect(page.getByRole('button', { name: /Minimap/ })).toBeVisible();
      await shot(page, 'mine', scheme);
      await page.getByRole('button', { name: /Pixel Shaders/ }).click();
      await shot(page, 'mine-native', scheme);
      await page.getByRole('button', { name: /Day & Night Sky/ }).click();
      await shot(page, 'mine-dependency', scheme);
      await page.getByRole('tab', { name: 'Mod list' }).click();
      await expect(page.getByRole('heading', { name: 'Sky Core' })).toBeVisible();
      await shot(page, 'browse', scheme);
      await page.getByRole('button', { name: 'Pet Companions', exact: true }).click();
      await shot(page, 'browse-detail', scheme);
      await page.getByRole('button', { name: 'Mod list' }).click();
      await page.getByRole('button', { name: 'Install Sky Core' }).click();
      await expect(page.getByRole('dialog')).toBeVisible();
      await shot(page, 'install-plan', scheme);
      await page.getByRole('button', { name: 'Next' }).click();
      await shot(page, 'install-perms', scheme);
      await page.getByRole('button', { name: 'Install and turn on' }).click();
      await expect(page.getByRole('heading', { name: /is ready/ })).toBeVisible();
      await shot(page, 'install-done', scheme);
      await page.getByRole('button', { name: 'Close', exact: true }).last().click();
      await page.getByRole('button', { name: 'Install Turbo Lighting' }).click();
      await page.getByRole('button', { name: 'Install and turn on' }).click();
      await shot(page, 'install-native', scheme);
      await page.getByRole('button', { name: 'Don’t install' }).click();
      await page.getByRole('button', { name: 'Install Key Hints' }).click();
      await shot(page, 'install-newer', scheme);
      await page.getByRole('button', { name: 'Cancel' }).click();
      await page.getByRole('button', { name: 'Install from file or link' }).click();
      await page.getByLabel('Or paste a link').fill('https://example.org/pixel-shaders-0.7.2.bhmod');
      await shot(page, 'install-source', scheme);
      await page.getByRole('button', { name: 'Next' }).click();
      await expect(page.getByRole('heading', { name: 'The file check failed' })).toBeVisible();
      await shot(page, 'install-checkfailed', scheme);
      await page.keyboard.press('Escape');
      await page.getByRole('tab', { name: /Server mods/ }).click();
      await expect(page.getByText('mira.example.net:15151')).toBeVisible();
      await shot(page, 'servers', scheme);
    });

    test('mods states and small window', async ({ page }) => {
      test.setTimeout(120_000);
      await open(page, 'mods=empty', scheme);
      await page.getByRole('button', { name: /^Mods/ }).click();
      await expect(page.getByRole('heading', { name: 'No mods yet' })).toBeVisible();
      await shot(page, 'mine-empty', scheme);
      await open(page, 'mods=error', scheme);
      await page.getByRole('button', { name: /^Mods/ }).click();
      await expect(page.getByRole('alert')).toBeVisible();
      await shot(page, 'mine-error', scheme);
      await open(page, 'modsindex=offline', scheme);
      await page.getByRole('button', { name: /^Mods/ }).click();
      await page.getByRole('tab', { name: 'Mod list' }).click();
      await expect(page.getByText('Can’t reach the mod list')).toBeVisible();
      await shot(page, 'browse-offline', scheme);
      await open(page, 'dev=1', scheme);
      await page.getByRole('button', { name: /^Mods/ }).click();
      await page.getByRole('tab', { name: 'Developer' }).click();
      await expect(page.getByText('Developer mode is on')).toBeVisible();
      await shot(page, 'dev', scheme);
      await open(page, 'x=1', scheme, { width: 800, height: 600 });
      await page.getByRole('button', { name: /^Mods/ }).click();
      await shot(page, 'small-list', scheme);
      await page.getByRole('button', { name: /Zoom Out/ }).click();
      await shot(page, 'small-detail', scheme);
      await open(page, 'x=1', scheme);
      await page.getByRole('button', { name: 'Settings', exact: true }).first().click();
      await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'Mods' }).click();
      await expect(page.getByText('Manage your mods in the Mods area')).toBeVisible();
      await shot(page, 'settings', scheme);
    });
  });
}
