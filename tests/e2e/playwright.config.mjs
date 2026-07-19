import { defineConfig } from "@playwright/test";

// Playwright config for the daedalus-web-server e2e suite. Targets a running
// server via DAEDALUS_WEB_BASE_URL; the specs themselves skip when it is unset.
export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: process.env.DAEDALUS_WEB_BASE_URL,
    trace: "on-first-retry",
  },
});
