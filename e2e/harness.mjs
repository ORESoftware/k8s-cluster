// Boot recipe for the akrion-web-server browser e2e.
//
// Chrome discovery + the server lifecycle live here, next to the specs. The
// server is a Rust binary: prefer a prebuilt target/debug/akrion-web-server
// (CI builds it first), else fall back to `cargo run`.
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
  throw new Error(`web server did not become ready at ${url} within ${timeoutMs}ms`);
}

// Boots akrion-web-server on an ephemeral port and waits for /healthz.
// Set AKRION_WEB_TEST_URL to run against an already-running server.
export async function startServer() {
  const reuse = process.env.AKRION_WEB_TEST_URL;
  if (reuse) return { url: reuse.replace(/\/+$/, ""), stop: () => {} };

  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;

  const prebuilt = path.join(repoRoot, "target", "debug", "akrion-web-server");
  const useBinary = existsSync(prebuilt);
  const command = useBinary ? prebuilt : "cargo";
  const args = useBinary ? [] : ["run", "--quiet"];

  const child = spawn(command, args, {
    cwd: repoRoot,
    stdio: "ignore",
    detached: true,
    env: {
      ...process.env,
      HOST: "127.0.0.1",
      PORT: String(port),
      // No real backend needed for the pages under test; point at a dead URL so
      // any fragment fetch fails fast rather than hanging.
      AKRION_BACKEND_URL: process.env.AKRION_BACKEND_URL ?? "http://127.0.0.1:1",
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
    await waitForReady(`${url}/healthz`, useBinary ? 60000 : 300000);
  } catch (err) {
    stop();
    throw err;
  }

  return { url, stop };
}
