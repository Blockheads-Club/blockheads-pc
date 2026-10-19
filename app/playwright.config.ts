import { defineConfig } from '@playwright/test';

// PW_PORT lets parallel checkouts (agent worktrees) run the e2e tests side by side; scripts/check-full.sh picks a free one.
const port = Number(process.env.PW_PORT ?? 4173);

export default defineConfig({
  testDir: 'e2e',
  timeout: 40_000,
  use: { baseURL: `http://localhost:${port}`, viewport: { width: 1100, height: 760 } },
  webServer: { command: `bun run build && bun run preview --port ${port} --strictPort`, url: `http://localhost:${port}`, reuseExistingServer: !process.env.CI, timeout: 120_000 },
  projects: [{ name: 'desktop' }]
});
