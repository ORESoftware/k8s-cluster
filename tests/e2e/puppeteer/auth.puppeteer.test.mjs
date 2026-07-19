// Puppeteer e2e: the daedalus-web-server auth gate, via node:test.
//
// Same coverage intent as the Playwright spec, using Puppeteer to satisfy the
// "both Puppeteer and Playwright" requirement and to catch a headless-Chromium
// difference either tool alone might miss. Env-gated on DAEDALUS_WEB_BASE_URL:
//   DAEDALUS_WEB_BASE_URL=https://app.daedalus-fab.com \
//   DAEDALUS_WEB_TOKEN=<token> node --test tests/e2e/puppeteer
//
// Skips cleanly (not fails) when the base URL or puppeteer is unavailable, so
// the base `npm test` never depends on a browser download or a live server.

import test from "node:test";
import assert from "node:assert/strict";

const BASE = process.env.DAEDALUS_WEB_BASE_URL;
const TOKEN = process.env.DAEDALUS_WEB_TOKEN;

// Import puppeteer lazily; if it is not installed, skip rather than crash.
let puppeteer = null;
try {
  ({ default: puppeteer } = await import("puppeteer"));
} catch {
  puppeteer = null;
}

const runnable = Boolean(BASE && puppeteer);

test("health endpoint is reachable", { skip: !runnable && "no base url / puppeteer" }, async () => {
  const browser = await puppeteer.launch({ headless: "new" });
  try {
    const page = await browser.newPage();
    const response = await page.goto(`${BASE}/health`, { waitUntil: "domcontentloaded" });
    assert.ok(response.ok(), `expected 2xx from /health, got ${response.status()}`);
  } finally {
    await browser.close();
  }
});

test(
  "anonymous access to the plans page is refused",
  { skip: !runnable && "no base url / puppeteer" },
  async () => {
    const browser = await puppeteer.launch({ headless: "new" });
    try {
      const page = await browser.newPage();
      const response = await page.goto(`${BASE}/`, { waitUntil: "domcontentloaded" });
      // A 200 would mean the auth gate is not enforced.
      assert.notEqual(response.status(), 200, "anonymous access must not be served");
      assert.ok(
        [401, 503].includes(response.status()),
        `expected 401 or 503, got ${response.status()}`,
      );
    } finally {
      await browser.close();
    }
  },
);

test(
  "an allow-listed operator sees the plans page",
  { skip: (!runnable || !TOKEN) && "no token / base url / puppeteer" },
  async () => {
    const browser = await puppeteer.launch({ headless: "new" });
    try {
      const page = await browser.newPage();
      await page.setExtraHTTPHeaders({ authorization: `Bearer ${TOKEN}` });
      const response = await page.goto(`${BASE}/`, { waitUntil: "domcontentloaded" });
      assert.equal(response.status(), 200);
      const heading = await page.$eval("h1", (el) => el.textContent || "");
      assert.match(heading, /fabrication plans/i);
    } finally {
      await browser.close();
    }
  },
);
