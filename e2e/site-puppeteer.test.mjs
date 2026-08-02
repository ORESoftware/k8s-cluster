import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { chromeExecutablePath, startServer } from "./harness.mjs";

// Puppeteer driver — a second independent browser stack asserting the same
// dashboard invariants, so a driver-specific quirk can't hide a regression.
test("puppeteer: dashboard renders, htmx executes under CSP, ws connects", async (t) => {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await puppeteer.launch({
    executablePath: chromeExecutablePath(),
    headless: "new",
    // CI runs Chrome as root, where launch fails without --no-sandbox.
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  await page.setViewport({ height: 900, width: 1440 });

  const requested = [];
  page.on("request", (r) => requested.push(r.url()));
  const cspViolations = [];
  page.on("console", (m) => {
    if (m.text().includes("Content Security Policy")) cspViolations.push(m.text());
  });
  const wsUrls = [];
  const cdp = await page.target().createCDPSession();
  await cdp.send("Network.enable");
  cdp.on("Network.webSocketCreated", ({ url }) => wsUrls.push(url));

  const response = await page.goto(`${server.url}/`, { waitUntil: "networkidle0" });
  assert.equal(response.status(), 200);
  assert.equal(await page.title(), "t2v · Dashboard");

  // Hardening headers.
  const headers = response.headers();
  assert.match(headers["content-security-policy"] ?? "", /default-src 'self'/);
  assert.match(headers["content-security-policy"] ?? "", /script-src 'self'/);
  assert.equal(headers["x-frame-options"], "DENY");
  assert.equal(headers["x-content-type-options"], "nosniff");

  // Four unique stat cards.
  for (const id of ["stat-transcriptions", "stat-translations", "stat-syntheses", "stat-vapi"]) {
    const count = await page.$$eval(`#${id}`, (els) => els.length);
    assert.equal(count, 1, `#${id} must be a single node`);
  }

  // Vendored htmx executed under the CSP; no CDN.
  assert.ok(!requested.some((u) => u.includes("unpkg.com") || u.includes("cdn")), "no CDN contacted");
  const htmxVersion = await page.evaluate(() => globalThis.htmx?.version);
  assert.ok(htmxVersion, "htmx global should be defined (script executed under CSP)");
  assert.deepEqual(cspViolations, [], "no CSP violations");

  // Live-stats websocket connected.
  await new Promise((r) => setTimeout(r, 500));
  assert.ok(wsUrls.some((u) => u.includes("/ws/stats")), `ws not connected: ${JSON.stringify(wsUrls)}`);
});
