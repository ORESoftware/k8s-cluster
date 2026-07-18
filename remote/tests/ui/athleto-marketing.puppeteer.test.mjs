// Puppeteer-driven node:test smoke of the live AthletO marketing site. Mirrors
// the Playwright suite so a single-engine regression still fails CI, and adds
// the CSP-meta and console-error checks. Run: `pnpm run test:ui:athleto`.
import assert from "node:assert/strict";
import test, { after, before } from "node:test";

import { gotoWithRetry, launchPuppeteer, MARKETING_URL, statusOf } from "./lib/harness.mjs";

let driver;
before(async () => {
  driver = await launchPuppeteer();
  console.log(`[athleto-ui:puppeteer] marketing target=${MARKETING_URL} (engine=${driver.engine})`);
});
after(async () => {
  await driver?.close();
});

test("marketing home responds 200 with the AthletO title (puppeteer)", async () => {
  const { page } = await driver.newPage();
  const response = await gotoWithRetry(page, `${MARKETING_URL}/`);
  assert.equal(statusOf(response), 200);
  assert.match(await page.title(), /AthletO/i);
});

test("renders 10 product cards and 10 sampler badges (puppeteer)", async () => {
  const { page } = await driver.newPage();
  await gotoWithRetry(page, `${MARKETING_URL}/`);
  await page.waitForSelector("#products .product-card");
  const cards = await page.$$eval("#products .product-card", (els) => els.length);
  const badges = await page.$$eval(".sample-badge", (els) => els.length);
  assert.equal(cards, 10);
  assert.equal(badges, 10);
});

test("sampler swaps aria-pressed and the visible panel (puppeteer)", async () => {
  const { page } = await driver.newPage();
  await gotoWithRetry(page, `${MARKETING_URL}/`);
  await page.waitForSelector(".sampler-controls button[data-sample]");
  const before = await page.$$eval(".sampler-controls button[data-sample]", (btns) => ({
    count: btns.length,
    firstPressed: btns[0].getAttribute("aria-pressed"),
  }));
  assert.equal(before.count, 10);
  assert.equal(before.firstPressed, "true");

  // Click the second sampler button and read the resulting state.
  const result = await page.evaluate(() => {
    const btns = [...document.querySelectorAll(".sampler-controls button[data-sample]")];
    btns[1].click();
    const visible = [...document.querySelectorAll("[data-sample-card]")].filter((c) => !c.hidden);
    return {
      secondPressed: btns[1].getAttribute("aria-pressed"),
      firstPressed: btns[0].getAttribute("aria-pressed"),
      visibleCount: visible.length,
      visibleKey: visible[0]?.getAttribute("data-sample-card"),
      selectedKey: btns[1].getAttribute("data-sample"),
    };
  });
  assert.equal(result.secondPressed, "true");
  assert.equal(result.firstPressed, "false");
  assert.equal(result.visibleCount, 1);
  assert.equal(result.visibleKey, result.selectedKey);
});

test("serves a restrictive Content-Security-Policy meta tag (puppeteer)", async () => {
  const { page } = await driver.newPage();
  await gotoWithRetry(page, `${MARKETING_URL}/`);
  const csp = await page.$eval(
    'meta[http-equiv="content-security-policy" i]',
    (el) => el.getAttribute("content") ?? "",
  );
  assert.match(csp, /default-src 'none'/, "CSP should default-deny");
  assert.match(csp, /script-src [^;]*'sha256-/, "inline script must be hash-allowed, not blanket");
  assert.doesNotMatch(csp, /unsafe-inline/, "CSP must not allow unsafe-inline");
});

test("loads without first-party console errors (puppeteer)", async () => {
  const { page } = await driver.newPage();
  const errors = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(msg.text());
  });
  page.on("pageerror", (err) => errors.push(String(err)));
  await gotoWithRetry(page, `${MARKETING_URL}/`, { waitUntil: "networkidle2" });
  // Ignore third-party edge noise (Cloudflare analytics beacon, favicon probes)
  // so this asserts only on the site's own code.
  const firstParty = errors.filter(
    (e) => !/cloudflareinsights|cloudflare|favicon|beacon\.min\.js/i.test(e),
  );
  assert.deepEqual(firstParty, [], `first-party console errors:\n${firstParty.join("\n")}`);
});
