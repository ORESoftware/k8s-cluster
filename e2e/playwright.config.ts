import { defineConfig, devices } from "@playwright/test";

// Booted binary + port. CI builds `../target/release/t2v-web` first; override
// with T2V_WEB_BIN (e.g. a debug build) locally.
const PORT = process.env.T2V_WEB_PORT ?? "8231";
const baseURL = `http://127.0.0.1:${PORT}`;
const bin = process.env.T2V_WEB_BIN ?? "../target/release/t2v-web";

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: { baseURL, trace: "on-first-retry" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  // Playwright boots t2v-web against a throwaway SQLite DB (the migrator
  // self-provisions it) and waits for /healthz before the tests run. The API
  // base points at a dead port — the dashboard GET routes and assets under test
  // never call it; only the interactive translate/TTS proxy would.
  webServer: {
    command: bin,
    url: `${baseURL}/healthz`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    env: {
      HOST: "127.0.0.1",
      PORT,
      DATABASE_URL: "sqlite:./.e2e.sqlite?mode=rwc",
      API_BASE_URL: "http://127.0.0.1:9",
      RUST_LOG: "warn",
      // No OTEL collector in CI; keep telemetry export from noising up boot.
      OTEL_SDK_DISABLED: "true",
    },
  },
});
