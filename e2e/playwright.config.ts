import { defineConfig, devices } from "@playwright/test";
import { generateKeyPairSync } from "node:crypto";
import path from "node:path";

// The server is booted from the compiled release binary. CI runs
// `cargo build --release` first; override the binary with SHARED_AUTH_BIN
// (e.g. a debug build) locally.
const PORT = process.env.SHARED_AUTH_PORT ?? "18120";
const baseURL = `http://127.0.0.1:${PORT}`;
const repoRoot = path.resolve(__dirname, "..");
const bin = process.env.SHARED_AUTH_BIN ?? "./target/release/shared-auth-server";

// A throwaway ES256 (P-256) signing key, generated fresh each run. The server
// requires AUTH_SIGNING_KEY_PEM to boot; this key signs nothing anyone trusts —
// it exists only so the process can start and publish a JWKS for these UI
// smokes. Generated rather than committed so no private key ever lands in git
// (and the secret scanner stays quiet). Honour an externally supplied key when
// present (e.g. a CI job that provisions its own).
const signingPem =
  process.env.AUTH_SIGNING_KEY_PEM ??
  generateKeyPairSync("ec", {
    namedCurve: "prime256v1",
    privateKeyEncoding: { type: "pkcs8", format: "pem" },
    publicKeyEncoding: { type: "spki", format: "pem" },
  }).privateKey;

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
  // Boot shared-auth-server with the minimum viable config: a throwaway signing
  // key, no RDS mirror (AUTH_DATABASE_URL unset -> db=None), and no Supabase
  // projects. The UI smokes exercise rendering, security headers, and the
  // fail-closed exchange-error path — none of which need a real Supabase
  // upstream or database. `cwd` is the repo root so the binary finds
  // `.cli-flags.toml`.
  webServer: {
    command: bin,
    cwd: repoRoot,
    url: `${baseURL}/healthz`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    env: {
      AUTH_BIND_ADDR: `127.0.0.1:${PORT}`,
      AUTH_SIGNING_KEY_PEM: signingPem,
      // Explicit DB-less mode: the UI smokes never touch the RDS identity
      // mirror, so there is no need to provision a Postgres just to render pages.
      AUTH_ALLOW_DBLESS: "true",
      RUST_LOG: "warn",
      // No OTEL collector under test; keep exporter retries from noising boot.
      OTEL_SDK_DISABLED: "true",
    },
  },
});
