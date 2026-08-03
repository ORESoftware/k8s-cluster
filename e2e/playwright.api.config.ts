import { defineConfig, devices } from "@playwright/test";

const PORT = process.env.T2V_OPENAPI_PORT ?? "18130";
const baseURL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  testMatch: "api-docs.spec.ts",
  timeout: 30_000,
  expect: { timeout: 8_000 },
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: { baseURL, trace: "on-first-retry" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command:
      "cargo run --manifest-path ../Cargo.toml --locked -p t2v-api --example openapi_fixture",
    url: `${baseURL}/healthz`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    env: {
      T2V_OPENAPI_FIXTURE_ADDR: `127.0.0.1:${PORT}`,
      OTEL_SDK_DISABLED: "true",
      RUST_LOG: "warn",
    },
  },
});
