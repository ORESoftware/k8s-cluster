import { defineConfig } from '@playwright/test';

// The suite manages its own Chromium launches (proxy args depend on the
// runtime-allocated SOCKS port), so no global `use` browser config is needed.
export default defineConfig({
  testDir: '.',
  timeout: 120000,
  expect: { timeout: 15000 },
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
});
