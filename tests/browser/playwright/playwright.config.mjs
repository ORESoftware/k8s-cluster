import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  reporter: [['list']],
  use: {
    baseURL: (process.env.DD_FAB_E2E_BASE_URL ?? 'http://127.0.0.1:8115').replace(/\/+$/, ''),
    headless: true,
  },
});
