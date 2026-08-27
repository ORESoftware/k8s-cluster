// Shared harness for the hermetic browser E2E suite.
//
// Every test here drives real in-repo front-end assets (never a live
// deployment) through BOTH browser engines. The harness deliberately blocks
// non-loopback network requests, captures browser diagnostics, and writes
// failure artifacts when BROWSER_ARTIFACT_DIR (or withPage(..., { artifactDir }))
// is configured.

import { createServer } from "node:http";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
// remote/tests/browser -> repo root is three levels up.
export const repoRoot = path.resolve(here, "..", "..", "..");
export const fixturesDir = path.join(here, "fixtures");

const ALL_ENGINES = ["puppeteer", "playwright"];
export const ENGINES = (() => {
  const requested = (process.env.BROWSER_ENGINES ?? "")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  if (requested.length === 0) return ALL_ENGINES;
  const unknown = requested.filter((engine) => !ALL_ENGINES.includes(engine));
  if (unknown.length > 0) {
    throw new Error(`BROWSER_ENGINES has unknown engine(s): ${unknown.join(", ")}`);
  }
  return requested;
})();

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
};

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "::1", "[::1]"]);
const PASSIVE_PROTOCOLS = new Set(["about:", "blob:", "data:", "file:"]);
const NETWORK_PROTOCOLS = new Set(["http:", "https:", "ws:", "wss:"]);

function contentType(filePath) {
  return CONTENT_TYPES[path.extname(filePath)] ?? "application/octet-stream";
}

function browserUrlIsAllowed(rawUrl) {
  let parsed;
  try {
    parsed = new URL(rawUrl);
  } catch {
    return false;
  }
  if (PASSIVE_PROTOCOLS.has(parsed.protocol)) return true;
  if (!NETWORK_PROTOCOLS.has(parsed.protocol)) return false;
  return LOOPBACK_HOSTS.has(parsed.hostname);
}

function responseHeaders(type) {
  return {
    "cache-control": "no-store",
    "content-type": type,
    "cross-origin-resource-policy": "same-origin",
    "referrer-policy": "no-referrer",
    "service-worker-allowed": "/",
    "x-content-type-options": "nosniff",
  };
}

/**
 * Start a deterministic local-only static server.
 *
 * `routes` maps a URL pathname to either:
 *   - `{ file: "<absolute path>" }`, or
 *   - `{ body: "<string>", type: "<content-type>" }`.
 *
 * Only GET and HEAD are accepted. Unknown routes return 404 and malformed
 * request targets return 400. The server binds to 127.0.0.1 on an ephemeral
 * port so parallel test files cannot collide.
 */
export async function startStaticServer(routes) {
  const server = createServer(async (req, res) => {
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.writeHead(405, {
        allow: "GET, HEAD",
        ...responseHeaders("text/plain; charset=utf-8"),
      });
      res.end("method not allowed");
      return;
    }

    let urlPath;
    try {
      urlPath = new URL(req.url ?? "/", "http://127.0.0.1").pathname;
    } catch {
      res.writeHead(400, responseHeaders("text/plain; charset=utf-8"));
      res.end("invalid request target");
      return;
    }

    const route = routes[urlPath];
    if (!route) {
      res.writeHead(404, responseHeaders("text/plain; charset=utf-8"));
      res.end(`no route for ${urlPath}`);
      return;
    }

    try {
      let data;
      let type;
      if (route.file) {
        data = await readFile(route.file);
        type = route.type ?? contentType(route.file);
      } else {
        data = route.body ?? "";
        type = route.type ?? "text/plain; charset=utf-8";
      }
      res.writeHead(200, responseHeaders(type));
      if (req.method === "HEAD") {
        res.end();
      } else {
        res.end(data);
      }
    } catch (error) {
      res.writeHead(500, responseHeaders("text/plain; charset=utf-8"));
      res.end(error instanceof Error ? error.message : String(error));
    }
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });

  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("static server did not expose a TCP address");
  }

  return {
    origin: `http://127.0.0.1:${address.port}`,
    async close() {
      server.closeIdleConnections?.();
      server.closeAllConnections?.();
      await new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
    },
  };
}

/** True if the repo file exists in this checkout (submodule may be uninitialised). */
export function assetExists(relativePath) {
  return existsSync(path.join(repoRoot, relativePath));
}

/** Poll `page.evaluate(predicate)` until it returns truthy. */
export async function pollUntil(page, predicate, { timeout = 10_000, interval = 100 } = {}) {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await page.evaluate(predicate);
    if (value) return value;
    if (Date.now() > deadline) throw new Error("pollUntil timed out");
    await new Promise((resolve) => setTimeout(resolve, interval));
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
    const { chromium } = await import("playwright");
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`[harness] puppeteer default launch failed (${message}); using playwright chromium`);
    return puppeteer.launch({
      headless: true,
      executablePath: chromium.executablePath(),
      args: LAUNCH_ARGS,
    });
  }
}

function createDiagnostics() {
  return {
    blockedRequests: [],
    console: [],
    pageErrors: [],
    requestFailures: [],
  };
}

function attachDiagnostics(page, diagnostics) {
  page.on("console", (message) => {
    diagnostics.console.push({ type: message.type(), text: message.text() });
  });
  page.on("pageerror", (error) => {
    diagnostics.pageErrors.push(error instanceof Error ? error.stack ?? error.message : String(error));
  });
  page.on("requestfailed", (request) => {
    diagnostics.requestFailures.push({
      errorText: request.failure()?.errorText ?? "unknown",
      method: request.method(),
      url: request.url(),
    });
  });
}

async function enforcePuppeteerNetworkPolicy(page, diagnostics, allowExternalRequests) {
  if (allowExternalRequests) return;
  await page.setRequestInterception(true);
  page.on("request", (request) => {
    if (browserUrlIsAllowed(request.url())) {
      void request.continue();
      return;
    }
    diagnostics.blockedRequests.push(request.url());
    void request.abort("blockedbyclient");
  });
}

async function enforcePlaywrightNetworkPolicy(context, diagnostics, allowExternalRequests) {
  if (allowExternalRequests) return;
  await context.route("**/*", async (route) => {
    const requestUrl = route.request().url();
    if (browserUrlIsAllowed(requestUrl)) {
      await route.continue();
      return;
    }
    diagnostics.blockedRequests.push(requestUrl);
    await route.abort("blockedbyclient");
  });
}

function sanitizeArtifactName(value) {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "browser-test";
}

async function captureFailureArtifacts(page, engine, diagnostics, options) {
  const configured = options.artifactDir ?? process.env.BROWSER_ARTIFACT_DIR;
  if (!configured) return null;

  const artifactDir = path.resolve(configured);
  await mkdir(artifactDir, { recursive: true });
  const stem = [
    sanitizeArtifactName(options.artifactName ?? "browser-test"),
    engine,
    process.pid,
    Date.now(),
  ].join("-");

  const screenshotPath = path.join(artifactDir, `${stem}.png`);
  const diagnosticsPath = path.join(artifactDir, `${stem}.json`);
  await Promise.allSettled([
    page.screenshot({ path: screenshotPath, fullPage: true }),
    writeFile(diagnosticsPath, `${JSON.stringify(diagnostics, null, 2)}\n`, "utf8"),
  ]);
  return { artifactDir, stem };
}

function enrichError(error, diagnostics) {
  const summary = {
    blockedRequests: diagnostics.blockedRequests,
    consoleErrors: diagnostics.console.filter((entry) => entry.type === "error"),
    pageErrors: diagnostics.pageErrors,
    requestFailures: diagnostics.requestFailures,
  };
  const suffix = `\nBrowser diagnostics: ${JSON.stringify(summary)}`;
  if (error instanceof Error) {
    error.message += suffix;
    return error;
  }
  return new Error(`${String(error)}${suffix}`);
}

/**
 * Open a browser, apply the hermetic network policy, hand a normalized page to
 * `fn`, and always tear the browser down. On failure, a screenshot and JSON
 * diagnostics are retained when an artifact directory is configured. Playwright
 * failures additionally retain a trace ZIP.
 */
export async function withPage(engine, fn, options = {}) {
  const diagnostics = createDiagnostics();
  const allowExternalRequests = options.allowExternalRequests === true;
  const failOnConsoleError = options.failOnConsoleError === true;

  if (engine === "puppeteer") {
    const browser = await launchPuppeteer();
    try {
      const page = await browser.newPage();
      attachDiagnostics(page, diagnostics);
      await enforcePuppeteerNetworkPolicy(page, diagnostics, allowExternalRequests);
      try {
        const result = await fn(wrap("puppeteer", page, diagnostics));
        if (failOnConsoleError && diagnostics.console.some((entry) => entry.type === "error")) {
          throw new Error("browser emitted console.error output");
        }
        return result;
      } catch (error) {
        await captureFailureArtifacts(page, engine, diagnostics, options);
        throw enrichError(error, diagnostics);
      }
    } finally {
      await browser.close();
    }
  }

  const { chromium } = await import("playwright");
  const browser = await chromium.launch({ headless: true, args: LAUNCH_ARGS });
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  let traceStarted = false;
  try {
    await enforcePlaywrightNetworkPolicy(context, diagnostics, allowExternalRequests);
    const configuredArtifactDir = options.artifactDir ?? process.env.BROWSER_ARTIFACT_DIR;
    if (configuredArtifactDir) {
      await context.tracing.start({ screenshots: true, snapshots: true, sources: true });
      traceStarted = true;
    }
    const page = await context.newPage();
    attachDiagnostics(page, diagnostics);
    try {
      const result = await fn(wrap("playwright", page, diagnostics));
      if (failOnConsoleError && diagnostics.console.some((entry) => entry.type === "error")) {
        throw new Error("browser emitted console.error output");
      }
      if (traceStarted) {
        await context.tracing.stop();
        traceStarted = false;
      }
      return result;
    } catch (error) {
      const artifact = await captureFailureArtifacts(page, engine, diagnostics, options);
      if (traceStarted) {
        if (artifact) {
          await context.tracing.stop({ path: path.join(artifact.artifactDir, `${artifact.stem}.zip`) });
        } else {
          await context.tracing.stop();
        }
        traceStarted = false;
      }
      throw enrichError(error, diagnostics);
    }
  } finally {
    if (traceStarted) await context.tracing.stop().catch(() => {});
    await context.close();
    await browser.close();
  }
}

function wrap(engine, page, diagnostics) {
  return {
    diagnostics,
    engine,
    raw: page,
    goto: (url, options = {}) =>
      page.goto(url, { waitUntil: "domcontentloaded", timeout: 30_000, ...options }),
    title: () => page.title(),
    waitForSelector: (selector, options = {}) =>
      page.waitForSelector(selector, { timeout: 15_000, ...options }),
    evaluate: (fn, ...args) => page.evaluate(fn, ...args),
    text: (selector) =>
      engine === "playwright"
        ? page.textContent(selector)
        : page.$eval(selector, (element) => element.textContent).catch(() => null),
    select: (selector, value) =>
      engine === "playwright" ? page.selectOption(selector, value) : page.select(selector, value),
    fill: async (selector, value) => {
      if (engine === "playwright") {
        await page.fill(selector, value);
        await page.dispatchEvent(selector, "change");
        return;
      }
      await page.$eval(
        selector,
        (element, nextValue) => {
          element.focus();
          element.value = nextValue;
          element.dispatchEvent(new Event("input", { bubbles: true }));
          element.dispatchEvent(new Event("change", { bubbles: true }));
        },
        value,
      );
    },
    click: (selector) => page.click(selector),
  };
}
