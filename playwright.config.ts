import { defineConfig, devices } from "@playwright/test";

// Browser tests run against the real production build served by `astro preview`
// — the same static output that is uploaded to GitHub Pages — not the dev
// server, so the production-only `<meta>` CSP and the built CSS are exercised.
// A dedicated port, not Astro's default 4321: any other dev server on the
// machine may already hold 4321, and `reuseExistingServer` would then silently
// point the whole suite at somebody else's site. For the same reason the server
// is never reused — if the port is busy, `astro preview` fails loudly instead.
const PORT = Number(process.env.SONUS_AURIS_E2E_PORT ?? 4327);
const BASE_URL = `http://127.0.0.1:${PORT}`;

// Deployment-configured URLs are fixtures, not real endpoints: the suite must
// exercise the "downloads are configured" rendering path (store badges as
// links, desktop installer buttons, checksum links) deterministically, whether
// or not the repository happens to define the corresponding variables. Nothing
// is ever fetched from these hosts — the suite asserts the page makes no
// cross-origin requests at all.
const downloadFixtureEnv = {
  PUBLIC_APP_STORE_URL: "https://apps.apple.com/app/sonus-auris/id0000000000",
  PUBLIC_PLAY_STORE_URL:
    "https://play.google.com/store/apps/details?id=app.sonusauris",
  PUBLIC_DOWNLOAD_BASE_URL: "https://downloads.sonusauris.app",
  PUBLIC_RUST_DESKTOP_DOWNLOAD_BASE_URL:
    "https://downloads.sonusauris.app/desktop/alpha/latest",
  PUBLIC_FLUTTER_DESKTOP_DOWNLOAD_BASE_URL:
    "https://downloads.sonusauris.app/desktop/flutter-alpha/latest",
  PUBLIC_DESKTOP_TESTERS_URL: "https://drive.google.com/drive/folders/example",
};

export default defineConfig({
  testDir: "tests/browser",
  outputDir: "test-results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: process.env.CI ? 2 : undefined,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  reporter: process.env.CI
    ? [["github"], ["html", { open: "never" }], ["list"]]
    : [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "off",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "npm run build && npm run preview",
    url: BASE_URL,
    reuseExistingServer: false,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
    env: {
      ...downloadFixtureEnv,
      // Read by astro.config.mjs for both `build` and `preview`.
      SONUS_AURIS_SITE_PORT: String(PORT),
    },
  },
});
