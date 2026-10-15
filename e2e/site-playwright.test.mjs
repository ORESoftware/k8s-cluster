import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startServer, HARDENING_HEADERS } from "./harness.mjs";

// Playwright driver. Runs the t2v-web dashboard in real Chromium and asserts
// the things only a browser can see: the vendored htmx actually executes under
// the CSP, the live-stats websocket connects, and no CDN is contacted.
test("playwright: dashboard renders, htmx executes under CSP, ws connects", async (t) => {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
  const requested = [];
  page.on("request", (r) => requested.push(r.url()));
  const cspViolations = [];
  page.on("console", (m) => {
    if (m.text().includes("Content Security Policy")) cspViolations.push(m.text());
  });
  const wsPromise = page.waitForEvent("websocket", { timeout: 8000 });

  const response = await page.goto(`${server.url}/`, { waitUntil: "networkidle" });
  assert.equal(response.status(), 200);
  assert.match(await page.title(), /t2v/i);

  // Hardening headers.
  const headers = response.headers();
  for (const [name, re] of Object.entries(HARDENING_HEADERS)) {
    assert.match(headers[name] ?? "", re, `${name} header`);
  }
  assert.ok(!(headers["content-security-policy"] ?? "").includes("unpkg"), "CSP must not allow a CDN");

  // Four live stat cards, each a unique element (regression: the ws OOB swap
  // used to leave two nodes with the same id).
  for (const id of ["#stat-transcriptions", "#stat-translations", "#stat-syntheses", "#stat-vapi"]) {
    assert.equal(await page.locator(id).count(), 1, `${id} must be a single node`);
    await assert.doesNotReject(page.locator(id).waitFor({ state: "visible" }));
  }

  // Vendored htmx loaded from our origin and EXECUTED under script-src 'self'.
  assert.ok(!requested.some((u) => u.includes("unpkg.com") || u.includes("cdn")), "no CDN contacted");
  assert.ok(requested.some((u) => u.endsWith("/assets/htmx.min.js")), "htmx served self-host");
  const htmxVersion = await page.evaluate(() => globalThis.htmx?.version);
  assert.ok(htmxVersion, "htmx global should be defined (script executed under CSP)");
  assert.deepEqual(cspViolations, [], "no CSP violations");

  // Live-stats websocket connected to the same-origin endpoint.
  const ws = await wsPromise;
  assert.match(ws.url(), /\/ws\/stats/);
});

test("playwright: nav to translate + speak renders interactive forms", async (t) => {
  const server = await startServer();
  t.after(() => server.stop());
  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());
  const page = await browser.newPage();

  await page.goto(`${server.url}/`, { waitUntil: "domcontentloaded" });
  await page.getByRole("link", { name: "Translate" }).click();
  await page.waitForURL(/\/translate$/);
  await page.locator('form[hx-post="/translate"]').waitFor({ state: "visible" });
  await page.locator('input[name="target_lang"]').waitFor({ state: "visible" });

  await page.getByRole("link", { name: "Text to Speech" }).click();
  await page.waitForURL(/\/speak$/);
  await page.locator('form[hx-post="/speak"]').waitFor({ state: "visible" });
});
