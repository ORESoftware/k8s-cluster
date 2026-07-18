import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startServer } from "./app-browser-harness.mjs";

// Playwright half of the web-server browser e2e: page-shell wiring, the JSON
// health/info API, and the per-route-class security headers. Runs the same
// SQLite-backed `serve` process as the puppeteer spec.

async function withPage(t) {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  return { server, page, pageErrors };
}

test("playwright: login shell wires htmx, the CSRF meta, and the client bundle without page errors", async (t) => {
  const { server, page, pageErrors } = await withPage(t);

  await page.goto(`${server.url}/login`, { waitUntil: "networkidle" });

  // Page-shell metadata rendered by the maud layout.
  assert.equal(
    await page.locator('meta[name="color-scheme"]').getAttribute("content"),
    "light dark",
  );
  // The login form carries a CSRF token in a hidden field (the layout only
  // emits a csrf <meta> for authenticated pages, so assert the field here).
  assert.ok(
    (await page.locator('form input[name="csrf"]').getAttribute("value"))?.length > 0,
  );

  // The shell references the ES-module client bundle and the /app-assets mount
  // exists (a missing asset 404s instead of falling through to marketing HTML).
  assert.equal(
    await page.locator('script[type="module"]').getAttribute("src"),
    "/app-assets/app.js",
  );
  const missingAsset = await page.request.get(`${server.url}/app-assets/not-a-real-asset.js`);
  assert.equal(missingAsset.status(), 404);

  // htmx target slot for the async login result.
  await page.locator("#login-result").waitFor({ state: "attached" });
  assert.deepEqual(pageErrors, []);
});

test("playwright: the versioned JSON API reports health and the service info contract", async (t) => {
  const { server, page } = await withPage(t);

  const healthz = await page.request.get(`${server.url}/healthz`);
  assert.equal(healthz.status(), 200);

  const readyz = await page.request.get(`${server.url}/readyz`);
  assert.equal(readyz.status(), 200); // SQLite ping succeeds

  const health = await page.request.get(`${server.url}/api/v1/health`);
  assert.equal(health.status(), 200);
  assert.deepEqual(await health.json(), { status: "ok", service: "canonical-web-server" });

  const info = await page.request.get(`${server.url}/api/v1/info`);
  assert.equal(info.status(), 200);
  const body = await info.json();
  assert.equal(body.service, "canonical-web-server");
  assert.equal(body.domain, "canonical.cloud");
  assert.deepEqual(body.stack, ["supabase", "maud", "axum", "seaorm", "htmx"]);

  // Unknown API paths stay JSON and never fall into the marketing site.
  const missing = await page.request.get(`${server.url}/api/v1/missing`);
  assert.equal(missing.status(), 404);
  assert.equal((await missing.json()).error.code, "not_found");
});

test("playwright: application and marketing routes carry their tailored security headers", async (t) => {
  const { server, page } = await withPage(t);

  const appResponse = await page.request.get(`${server.url}/login`);
  const appHeaders = appResponse.headers();
  const marketingResponse = await page.request.get(`${server.url}/`);
  const marketingHeaders = marketingResponse.headers();

  // Global hardening applied to every response.
  for (const headers of [appHeaders, marketingHeaders]) {
    assert.equal(headers["x-content-type-options"], "nosniff");
    assert.equal(headers["x-frame-options"], "DENY");
    assert.match(headers["referrer-policy"], /strict-origin/);
  }

  // Both surfaces use same-origin external scripts. The marketing build has a
  // contract test preventing Astro regressions back to inline executable code.
  assert.match(appHeaders["content-security-policy"], /script-src 'self';/);
  assert.match(appHeaders["content-security-policy"], /connect-src 'self'/);
  assert.match(marketingHeaders["content-security-policy"], /script-src 'self';/);
  assert.doesNotMatch(appHeaders["content-security-policy"], /script-src 'self' 'unsafe-inline'/);
  assert.doesNotMatch(marketingHeaders["content-security-policy"], /script-src 'self' 'unsafe-inline'/);
});
