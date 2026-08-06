// Self-contained boot recipe for the canonical-web-server browser e2e.
//
// The router-level Rust tests (tests/app.rs) drive the app with tower's
// `oneshot` and never bind a socket. Puppeteer/Playwright need a real HTTP
// origin, so this harness compiles the binary once and runs `serve` exactly the
// way the container-smoke CI job does: explicitly migrate a temporary SQLite
// file, then serve it with a single pooled connection. No Postgres, Supabase,
// privileged runtime credential, or automatic migration path is involved.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import net from "node:net";

const REPO_ROOT = new URL("../../", import.meta.url).pathname;

// Resolve a Chrome/Chromium executable for Playwright/Puppeteer. Prefer an
// explicit env var (Nix dev shell / CI), then well-known system paths.
// Returning `undefined` lets each driver fall back to its own managed build.
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
  let lastError;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.status === 200) return;
    } catch (error) {
      lastError = error;
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`web server did not become ready at ${url} within ${timeoutMs}ms: ${lastError ?? ""}`);
}

// Compile the server (debug) unless CANONICAL_WEB_SERVER_BIN points at a prebuilt
// binary. The first compile is slow, so it happens here — outside the per-test
// readiness timeout — rather than while a test is waiting on the socket.
function resolveBinary() {
  const override = process.env.CANONICAL_WEB_SERVER_BIN;
  if (override) {
    if (!existsSync(override)) {
      throw new Error(`CANONICAL_WEB_SERVER_BIN does not exist: ${override}`);
    }
    return override;
  }
  const built = join(REPO_ROOT, "target", "debug", "canonical-web-server");
  const build = spawnSync(
    "cargo",
    [
      "build",
      "--locked",
      "--quiet",
      "--features",
      "test-auth",
      "--bin",
      "canonical-web-server",
    ],
    {
      cwd: REPO_ROOT,
      stdio: "inherit",
    },
  );
  if (build.status !== 0) {
    throw new Error(`cargo build failed (exit ${build.status}); set CANONICAL_WEB_SERVER_BIN to skip`);
  }
  if (!existsSync(built)) {
    throw new Error(`expected a built binary at ${built}`);
  }
  return built;
}

// The client bundle is served at /app-assets/app.js and referenced by every
// maud page. Build it best-effort: a missing bundle only 404s that one script
// (no uncaught page error), so tests still run, but building keeps the pages
// faithful to production.
function ensureClientBundle() {
  if (process.env.CANONICAL_SKIP_CLIENT_BUILD === "1") return;
  if (existsSync(join(REPO_ROOT, "client", "dist", "app.js"))) return;
  if (!existsSync(join(REPO_ROOT, "client", "node_modules"))) return;
  spawnSync("npm", ["run", "build", "--prefix", "client"], {
    cwd: REPO_ROOT,
    stdio: "ignore",
  });
}

// A one-file static tree so the marketing fallback route (`/`) returns 200 and
// carries the marketing CSP, without building the whole Astro site here.
function makeStaticFixture() {
  const dir = mkdtempSync(join(tmpdir(), "canonical-static-"));
  writeFileSync(
    join(dir, "index.html"),
    "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>canonical.cloud</title></head><body><main><h1>canonical.cloud</h1></main></body></html>",
  );
  return dir;
}

// Boots `canonical-web-server serve` on an ephemeral port and waits for
// /healthz. Set CANONICAL_WEB_SERVER_TEST_URL to run against an already-running
// server (skips the build and boot entirely).
export async function startServer() {
  const reuse = process.env.CANONICAL_WEB_SERVER_TEST_URL;
  if (reuse) {
    return { url: reuse.replace(/\/+$/, ""), stop: () => {} };
  }

  const binary = resolveBinary();
  ensureClientBundle();
  const staticDir = makeStaticFixture();
  const databaseDir = mkdtempSync(join(tmpdir(), "canonical-browser-db-"));
  const databaseUrl = `sqlite://${join(databaseDir, "browser.sqlite")}?mode=rwc`;
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;

  const migration = spawnSync(binary, ["migrate"], {
    cwd: REPO_ROOT,
    stdio: "ignore",
    env: {
      ...process.env,
      RUST_LOG: "off",
      MIGRATION_DATABASE_URL: databaseUrl,
      MIGRATION_DATABASE_MAX_CONNECTIONS: "1",
    },
  });
  if (migration.status !== 0) {
    throw new Error(`database migration failed (exit ${migration.status})`);
  }

  const child = spawn(binary, ["serve"], {
    cwd: REPO_ROOT,
    stdio: "ignore",
    detached: true,
    env: {
      ...process.env,
      PORT: String(port),
      RUST_LOG: "off",
      APP_BASE_URL: url,
      APP_ALLOWED_ORIGINS: url,
      // Non-zero 32-byte base64 key (same value the CI smoke jobs use).
      APP_SESSION_ENCRYPTION_KEY: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
      COOKIE_SECURE: "false",
      APP_SESSION_COOKIE: "canonical_session",
      // The separate command above created this schema. The web process only
      // receives its unprivileged runtime-shaped connection setting.
      DATABASE_URL: databaseUrl,
      DATABASE_MAX_CONNECTIONS: "1",
      SUPABASE_URL: "http://127.0.0.1:54321",
      SUPABASE_PUBLISHABLE_KEY: "sb_publishable_ci_only",
      // The binary must also have been built with the debug-only `test-auth`
      // feature. Release builds reject that feature at compile time.
      CANONICAL_TEST_AUTH_ENABLED: "1",
      STATIC_DIR: staticDir,
      APP_ASSET_DIR: join(REPO_ROOT, "client", "dist"),
    },
  });
  const exited = new Promise((resolve) => child.once("exit", resolve));
  child.unref();

  const stop = async () => {
    if (child.pid !== undefined && child.exitCode === null) {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch {
        try {
          child.kill("SIGTERM");
        } catch {
          // already gone
        }
      }
      await Promise.race([
        exited,
        new Promise((resolve) => setTimeout(resolve, 5000)),
      ]);
    }
    try {
      rmSync(staticDir, { recursive: true, force: true });
    } catch {
      // best effort
    }
    try {
      rmSync(databaseDir, { recursive: true, force: true });
    } catch {
      // best effort
    }
  };

  try {
    await waitForReady(`${url}/healthz`, 60000);
  } catch (error) {
    await stop();
    throw error;
  }

  return { url, stop };
}
