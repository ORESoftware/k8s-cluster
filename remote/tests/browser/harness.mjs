// Shared harness for the hermetic browser E2E suite.
//
// Every test here drives *real* in-repo front-end assets (never a live
// deployment) through BOTH browser engines — Puppeteer and Playwright — so a
// feature is proven to work the same way under each. A tiny static file server
// serves the asset under test from its actual path in the repo (plus a couple
// of in-memory fixtures), and `withPage()` gives each test a page on the engine
// it asked for. Node's built-in `node:test` is the runner.
//
// Design notes:
//   * localhost (127.0.0.1) is a "secure context", so Service Workers register
//     over plain http here — no TLS setup needed.
//   * Puppeteer's bundled Chromium is used when present; if the download was
//     skipped we fall back to Playwright's Chromium binary (the same fallback
//     the existing ui-*-smoke.mjs scripts use), so CI needs only one browser
//     download to run both engines.

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
// remote/tests/browser -> repo root is three levels up.
export const repoRoot = path.resolve(here, "..", "..", "..");
export const fixturesDir = path.join(here, "fixtures");

/** The two engines every scenario runs against. */
export const ENGINES = ["puppeteer", "playwright"];

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
};

function contentType(filePath) {
  return CONTENT_TYPES[path.extname(filePath)] ?? "application/octet-stream";
}

/**
 * Start a static server.
 *
 * `routes` maps a URL path to either:
 *   - `{ file: "<absolute path>" }`  — stream a real file from disk, or
 *   - `{ body: "<string>", type: "<content-type>" }` — an in-memory fixture.
 *
 * Anything not in `routes` gets a 404. Bind to 127.0.0.1:0 so the OS hands us a
 * free port and parallel test files never collide.
 */
export async function startStaticServer(routes) {
  const server = createServer(async (req, res) => {
    const urlPath = (req.url ?? "/").split("?")[0];
    const route = routes[urlPath];
    if (!route) {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end(`no route for ${urlPath}`);
      return;
    }
    try {
      if (route.file) {
        const data = await readFile(route.file);
        res.writeHead(200, {
          "content-type": route.type ?? contentType(route.file),
          // Service worker scripts must never be cached across a test run.
          "cache-control": "no-store",
          // Allow a root-scoped worker regardless of where the script sits.
          "service-worker-allowed": "/",
        });
        res.end(data);
        return;
      }
      res.writeHead(200, {
        "content-type": route.type ?? "text/plain; charset=utf-8",
        "cache-control": "no-store",
      });
      res.end(route.body ?? "");
    } catch (error) {
      res.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      res.end(error instanceof Error ? error.message : String(error));
    }
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const addr = server.address();
  const origin = `http://127.0.0.1:${addr.port}`;
  return {
    origin,
    async close() {
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

/** True if the repo file exists in this checkout (submodule may be uninitialised). */
export function assetExists(relPath) {
  return existsSync(path.join(repoRoot, relPath));
}

/**
 * Poll `page.evaluate(pred)` until it returns truthy or the timeout elapses.
 * Engine-agnostic (Puppeteer and Playwright order `waitForFunction`'s options
 * and args differently), so tests use this instead. Returns the truthy value.
 */
export async function pollUntil(page, pred, { timeout = 10_000, interval = 100 } = {}) {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await page.evaluate(pred);
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error("pollUntil timed out");
    }
    await new Promise((r) => setTimeout(r, interval));
  }
}

const LAUNCH_ARGS = [
  "--no-sandbox",
  "--disable-setuid-sandbox",
  "--disable-dev-shm-usage",
  "--ignore-certificate-errors",
];

async function launchPuppeteer() {
  const puppeteer = (await import("puppeteer")).default;
  try {
    return await puppeteer.launch({ headless: true, args: LAUNCH_ARGS });
  } catch (error) {
    // Puppeteer's own Chromium download was skipped — borrow Playwright's.
    const { chromium } = await import("playwright");
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`[harness] puppeteer default launch failed (${message}); using playwright chromium`);
    return await puppeteer.launch({
      headless: true,
      executablePath: chromium.executablePath(),
      args: LAUNCH_ARGS,
    });
  }
}

/**
 * Open a browser on `engine`, hand a page to `fn`, and always tear the browser
 * down afterwards. The page object is the engine's native Page — both expose
 * the same `goto`/`title`/`waitForSelector`/`evaluate` surface this suite uses;
 * the few divergent helpers are normalised by the `page.*` wrappers below.
 */
export async function withPage(engine, fn) {
  if (engine === "puppeteer") {
    const browser = await launchPuppeteer();
    try {
      const page = await browser.newPage();
      return await fn(wrap("puppeteer", page));
    } finally {
      await browser.close();
    }
  }
  const { chromium } = await import("playwright");
  const browser = await chromium.launch({ headless: true, args: LAUNCH_ARGS });
  try {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const page = await context.newPage();
    return await fn(wrap("playwright", page));
  } finally {
    await browser.close();
  }
}

// Normalise the small set of methods whose signatures differ across engines.
// Everything else (goto/title/evaluate/waitForSelector) is call-compatible.
function wrap(engine, page) {
  return {
    engine,
    raw: page,
    goto: (url, opts = {}) => page.goto(url, { waitUntil: "domcontentloaded", timeout: 30_000, ...opts }),
    title: () => page.title(),
    waitForSelector: (sel, opts = {}) => page.waitForSelector(sel, { timeout: 15_000, ...opts }),
    evaluate: (fn, ...args) => page.evaluate(fn, ...args),
    // textContent of the first match, or null.
    text: (sel) =>
      engine === "playwright"
        ? page.textContent(sel)
        : page.$eval(sel, (el) => el.textContent).catch(() => null),
    select: (sel, value) => (engine === "playwright" ? page.selectOption(sel, value) : page.select(sel, value)),
    fill: (sel, value) => (engine === "playwright" ? page.fill(sel, value) : page.$eval(sel, (el, v) => { el.value = v; }, value)),
    click: (sel) => page.click(sel),
  };
}
