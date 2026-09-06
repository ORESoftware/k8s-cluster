// Boot recipe for the t2v-web browser e2e (fleet convention: node --test +
// playwright/puppeteer/selenium as libraries, sharing this harness).
//
// The server is the Rust t2v-web binary: prefer a prebuilt
// target/release/t2v-web (CI builds it first), else target/debug/t2v-web, else
// `cargo run`. It self-provisions a throwaway SQLite DB via its migrator, so no
// Postgres or t2v-api is needed for the pages under test.
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");

export function chromeExecutablePath() {
  const fromEnv =
    process.env.PLAYWRIGHT_CHROMIUM ||
    process.env.PUPPETEER_EXECUTABLE_PATH ||
    process.env.CHROME_PATH ||
    process.env.CHROMIUM_PATH;
  if (fromEnv && existsSync(fromEnv)) return fromEnv;

  const candidates = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  return undefined;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

async function waitForReady(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.ok || res.status === 404) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`t2v-web did not become ready at ${url} within ${timeoutMs}ms`);
}

// Boots t2v-web on an ephemeral port and waits for /healthz.
// Set T2V_WEB_TEST_URL to run against an already-running server.
export async function startServer() {
  const reuse = process.env.T2V_WEB_TEST_URL;
  if (reuse) return { url: reuse.replace(/\/+$/, ""), stop: () => {} };

  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;

  const release = path.join(repoRoot, "target", "release", "t2v-web");
  const debug = path.join(repoRoot, "target", "debug", "t2v-web");
  const prebuilt = existsSync(release) ? release : existsSync(debug) ? debug : undefined;
  const command = prebuilt ?? "cargo";
  const args = prebuilt ? [] : ["run", "--quiet", "--bin", "t2v-web"];

  const child = spawn(command, args, {
    cwd: repoRoot,
    stdio: "ignore",
    detached: true,
    env: {
      ...process.env,
      HOST: "127.0.0.1",
      PORT: String(port),
      // Throwaway SQLite; the migrator self-provisions it.
      DATABASE_URL: `sqlite:${path.join(here, ".e2e.sqlite")}?mode=rwc`,
      // No t2v-api needed for the pages under test; point at a dead port so any
      // interactive proxy fails fast rather than hanging.
      API_BASE_URL: process.env.API_BASE_URL ?? "http://127.0.0.1:1",
      RUST_LOG: "warn",
      // No OTEL collector in CI; keep telemetry export from noising up boot.
      OTEL_SDK_DISABLED: "true",
    },
  });
  child.unref();

  const stop = () => {
    if (child.pid === undefined) return;
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      try {
        child.kill("SIGTERM");
      } catch {
        // already gone
      }
    }
  };

  try {
    // Generous: a cold `cargo run` compiles first.
    await waitForReady(`${url}/healthz`, prebuilt ? 60000 : 300000);
  } catch (err) {
    stop();
    throw err;
  }

  return { url, stop };
}

// Shared expectations, asserted identically by every driver so the three
// browsers agree. Each entry is (label, async (driver) => void) where `driver`
// exposes: title(), headers(url), evalHtmxVersion(), navText(), locateVisible(sel).
export const HARDENING_HEADERS = {
  "content-security-policy": /default-src 'self'.*script-src 'self'/s,
  "x-content-type-options": /^nosniff$/,
  "x-frame-options": /^DENY$/,
  "referrer-policy": /^no-referrer$/,
};
