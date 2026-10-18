import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: 'e2e',
  timeout: 40_000,
  use: { baseURL: 'http://localhost:4173', viewport: { width: 1100, height: 760 } },
  webServer: { command: 'bun run build && bun run preview --port 4173 --strictPort', url: 'http://localhost:4173', reuseExistingServer: !process.env.CI, timeout: 120_000 },
  projects: [{ name: 'desktop' }]
});
