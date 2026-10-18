import { expect, test, type Page } from '@playwright/test';
import { mockTauri } from './mock';

test.beforeEach(async ({ page }) => {
  await mockTauri(page);
});

const noOverflow = (page: Page) =>
  page.evaluate(() => {
    const bodies = [...document.querySelectorAll('main, .content, .win')];
    return bodies.every((el) => el.scrollWidth <= el.clientWidth + 1);
  });

const calls = (page: Page, cmd: string) => page.evaluate((c) => (window as any).__calls.filter((x: unknown[]) => x[0] === c).length, cmd);

test('the main window is revealed only when the boot is done; a hung or failed boot stays on the splash', async ({ page }) => {
  await page.goto('/?splash=hang');
  await expect.poll(() => calls(page, 'splash_set')).toBeGreaterThan(0);
  expect(await calls(page, 'main_ready')).toBe(0);
  await expect(page.getByRole('status')).toHaveCount(0);
  await page.goto('/?splash=error');
  await expect.poll(() => calls(page, 'splash_set')).toBeGreaterThan(0);
  expect(await calls(page, 'main_ready')).toBe(0);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: /The Blockheads/ })).toBeVisible();
  expect(await calls(page, 'main_ready')).toBe(1);
  await page.goto('/?first=1');
  await expect(page.getByRole('heading', { name: 'Play The Blockheads on your computer' })).toBeVisible();
  expect(await calls(page, 'main_ready')).toBe(1);
});

test('first run to Home, then play, crash and play again', async ({ page }) => {
  await page.goto('/?first=1&crash=1');
  await page.getByLabel('I own the game').check();
  await page.getByRole('button', { name: 'Next' }).click();
  await page.getByRole('button', { name: 'Browse…' }).click();
  await expect(page.getByText('The Blockheads 1.7.6')).toBeVisible();
  for (let i = 0; i < 4; i++) await page.getByRole('button', { name: /^(Next|Skip)$/ }).last().click();
  await page.getByRole('button', { name: 'Start the Launcher' }).click();
  await expect(page.getByRole('status', { name: 'Game status: Ready to play' })).toBeVisible();

  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await expect(page.getByText('The game runs in its own window')).toBeVisible();
  // The mock game crashes after a few seconds: the launcher notices and goes back to Home.
  await expect(page.getByRole('alert')).toContainText('The game closed unexpectedly', { timeout: 15_000 });
  await expect(page.getByLabel('Last log lines')).toContainText('exit     code 139');
  await page.getByRole('button', { name: 'Copy log' }).click();
  const calls = await page.evaluate(() => (window as any).__calls.map((c: unknown[]) => c[0]));
  expect(calls).toEqual(expect.arrayContaining(['finish_setup', 'game_start', 'game_poll']));
});

test('a failed start is reported and Home stays', async ({ page }) => {
  await page.goto('/?failstart=1');
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('cannot start the game');
  await expect(page.getByRole('status', { name: 'Game status: Ready to play' })).toBeVisible();
});

test('language and theme switch and stay usable by keyboard', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByLabel('Launcher language').selectOption('tr');
  await expect(page.getByRole('heading', { name: 'Dil ve tema' })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', 'tr');
  await page.getByRole('radio', { name: 'Koyu' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.getByRole('radio', { name: 'Koyu' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('radio', { name: 'Sistem' })).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('html')).not.toHaveAttribute('data-theme', 'dark');
  await page.getByRole('button', { name: 'Ana sayfaya dön' }).click();
  await page.keyboard.press('Control+,');
  await expect(page.getByRole('heading', { name: 'Ayarlar' })).toBeVisible();
});

test('shortcuts: Enter plays, F1 and Esc, Ctrl+L', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('status', { name: 'Game status: Ready to play' })).toBeVisible();
  await page.keyboard.press('F1');
  await expect(page.getByRole('dialog', { name: 'How to join a server' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.keyboard.press('Control+l');
  await expect(page.getByRole('dialog', { name: 'Game log' })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.locator('body').press('Enter');
  await expect(page.getByText('The game runs in its own window')).toBeVisible();
});

test('works from 800 x 600 up on every screen', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 600 });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: /The Blockheads/ })).toBeVisible();
  expect(await noOverflow(page)).toBe(true);
  for (const name of ['Game & data', 'Controls', 'Updates & about']) {
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name }).click();
    expect(await noOverflow(page)).toBe(true);
    await page.getByRole('button', { name: 'Back to home' }).click();
  }
  await page.goto('/?first=1');
  for (let step = 1; step <= 6; step++) {
    expect(await noOverflow(page)).toBe(true);
    if (step === 1) await page.getByLabel('I own the game').check();
    if (step === 2) await page.getByRole('button', { name: 'Browse…' }).click();
    if (step === 5) await page.getByRole('button', { name: 'Skip' }).click();
    else if (step < 6) await page.getByRole('button', { name: 'Next' }).click();
  }
});

test('the window buttons talk to the window', async ({ page }) => {
  await page.goto('/?os=windows');
  await page.getByRole('button', { name: 'Minimize window' }).click();
  await page.getByRole('button', { name: 'Maximize window' }).click();
  const calls = await page.evaluate(() => (window as any).__calls.map((c: unknown[]) => c[0]));
  expect(calls).toEqual(expect.arrayContaining(['plugin:window|minimize', 'plugin:window|toggle_maximize']));
});

test('rounded frame, and the title bar buttons call the window API', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('.frame.round')).toBeVisible();
  const called = (cmd: string) => page.evaluate((c) => (window as any).__calls.some((x: unknown[]) => x[0] === c), cmd);
  await page.getByRole('button', { name: 'Minimize window' }).click();
  await page.getByRole('button', { name: /Maximize window/ }).click();
  await page.getByRole('button', { name: 'Close window' }).click();
  expect(await called('plugin:window|minimize')).toBe(true);
  expect(await called('plugin:window|toggle_maximize')).toBe(true);
  expect(await called('plugin:window|close')).toBe(true);
});
