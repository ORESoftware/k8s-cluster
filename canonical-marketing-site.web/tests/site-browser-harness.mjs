// Self-contained boot recipe for the marketing-site browser e2e.
//
// canonical-marketing-site.web has no shared test-config package, so Chrome
// discovery and the `astro preview` server lifecycle both live here, next to
// the specs that use them.
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import net from "node:net";

// Resolve a Chrome/Chromium executable for Playwright/Puppeteer.
//
// Prefer an explicit env var (set by the Nix dev shell or CI), then a few
// well-known system paths. Returning `undefined` lets Puppeteer fall back to its
// own downloaded browser; Playwright likewise falls back to its managed build.
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
      // server not up yet
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`site did not become ready at ${url} within ${timeoutMs}ms`);
}

// Boots `astro preview` on an ephemeral port and waits until it answers.
//
// The site is built with base `/` (astro.config.mjs), so pages are served at
// `${url}/`. Set CANONICAL_SITE_TEST_URL to run against an already-running site.
export async function startSite() {
  const reuse = process.env.CANONICAL_SITE_TEST_URL;
  if (reuse) {
    return { url: reuse.replace(/\/+$/, ""), stop: () => {} };
  }

  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  // `npm run preview` forks an `astro preview` grandchild. Spawn detached so the
  // child is a process-group leader, and use `stdio: "ignore"` so the grandchild
  // never inherits this process's stdout/stderr — otherwise it holds those pipes
  // open and node --test's per-file subprocess never exits (it hangs to the test
  // timeout even after the assertions pass).
  const child = spawn(
    "npm",
    ["run", "preview", "--", "--port", String(port), "--host", "127.0.0.1"],
    {
      cwd: new URL("..", import.meta.url).pathname,
      stdio: "ignore",
      detached: true,
    },
  );
  child.unref();

  const stop = () => {
    if (child.pid === undefined) return;
    // Kill the whole process group (npm + the astro grandchild). Negative pid
    // targets the group whose leader is `child` (created by detached: true).
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
    await waitForReady(`${url}/`, 45000);
  } catch (err) {
    stop();
    throw err;
  }

  return { url, stop };
}
