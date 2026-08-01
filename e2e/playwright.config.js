const { defineConfig } = require('@playwright/test');

module.exports = defineConfig({
  testDir: '.',
  testMatch: /webauthn\.spec\.js/,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  retries: 0,
  workers: 1,
  use: {
    baseURL: process.env.AUTH_BASE_URL || 'http://localhost:8120',
    browserName: 'chromium',
    trace: 'retain-on-failure',
  },
  reporter: [['line']],
});
