import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [sveltekit()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] }
  },
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    setupFiles: ['src/test-setup.ts'],
    coverage: {
      provider: 'v8',
      include: ['src/lib/**/*.{ts,svelte}', 'src/routes/**/*.{ts,svelte}'],
      exclude: ['src/**/*.test.ts', 'src/lib/mock-api.ts', 'src/lib/test-utils.ts', 'src/lib/types.ts', 'src/lib/mods/types.ts', 'src/lib/mods/mock.ts'],
      reporter: ['text', 'text-summary'],
      thresholds: { lines: 100, branches: 100, functions: 100, statements: 100 }
    }
  }
});
