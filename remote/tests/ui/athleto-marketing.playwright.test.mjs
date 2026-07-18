// Playwright-driven node:test smoke of the live AthletO marketing site
// (athleto.store -> GitHub Pages via Cloudflare). Run: `pnpm run test:ui:athleto`.
import assert from "node:assert/strict";
import test, { after, before } from "node:test";

import { gotoWithRetry, launchPlaywright, MARKETING_URL, statusOf } from "./lib/harness.mjs";

let driver;
before(async () => {
  driver = await launchPlaywright();
  console.log(`[athleto-ui:playwright] marketing target=${MARKETING_URL}`);
});
after(async () => {
  await driver?.close();
});

async function openHome() {
  const { page } = await driver.newPage();
  const response = await gotoWithRetry(page, `${MARKETING_URL}/`);
  return { page, response };
}

test("marketing home responds 200 with the AthletO title", async () => {
  const { page, response } = await openHome();
  assert.equal(statusOf(response), 200, "home should return 200");
  assert.match(await page.title(), /AthletO/i);
});

test("hero shows the brand promise heading", async () => {
  const { page } = await openHome();
  const heading = page.getByRole("heading", { level: 1 });
  await heading.first().waitFor();
  assert.match(await heading.first().innerText(), /Wobble hard\. Recover clean\./);
});

test("renders all 10 product cards, each with an ingredients list", async () => {
  const { page } = await openHome();
  await assert.doesNotReject(page.locator("#products .product-card").first().waitFor());
  assert.equal(await page.locator("#products .product-card").count(), 10);
  // Every flavor card exposes its ingredient panel (the feature the user asked for).
  assert.equal(await page.locator("#products .product-card .ingredients").count(), 10);
});

test("sampler buttons toggle aria-pressed and swap the visible panel", async () => {
  const { page } = await openHome();
  const buttons = page.locator(".sampler-controls button[data-sample]");
  const visibleCard = page.locator("[data-sample-card]:not([hidden])");
  await buttons.first().waitFor();
  assert.equal(await buttons.count(), 10);
  assert.equal(await buttons.first().getAttribute("aria-pressed"), "true");
  assert.equal(await visibleCard.count(), 1);

  const second = buttons.nth(1);
  const key = await second.getAttribute("data-sample");
  await second.click();
  assert.equal(await second.getAttribute("aria-pressed"), "true");
  assert.equal(await buttons.first().getAttribute("aria-pressed"), "false");
  assert.equal(await visibleCard.count(), 1);
  assert.equal(await visibleCard.getAttribute("data-sample-card"), key);
});

test("sweetener messaging is sugar-free (stevia + erythritol, no added sugar)", async () => {
  const { page } = await openHome();
  const body = (await page.locator("body").innerText()).toLowerCase();
  assert.match(body, /stevia/);
  assert.match(body, /erythritol/);
  assert.match(body, /no added sugar|never sugar|zero sugar|no sugar/);
});

test("every external retailer link opens in a new tab with rel=noopener", async () => {
  const { page } = await openHome();
  const links = await page.$$eval('a[href^="http://"], a[href^="https://"]', (anchors) =>
    anchors.map((a) => ({
      href: a.getAttribute("href"),
      target: a.getAttribute("target"),
      rel: (a.getAttribute("rel") ?? "").split(/\s+/),
    })),
  );
  assert.ok(links.length >= 40, `expected >=40 external links, saw ${links.length}`);
  const offenders = links.filter((l) => l.target !== "_blank" || !l.rel.includes("noopener"));
  assert.deepEqual(offenders, [], `links missing target=_blank/rel=noopener: ${JSON.stringify(offenders)}`);
});

test("does not scroll horizontally on a phone viewport", async () => {
  const { page } = await driver.newPage();
  await page.setViewportSize({ width: 390, height: 844 });
  await gotoWithRetry(page, `${MARKETING_URL}/`);
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  assert.ok(overflow <= 1, `phone viewport overflowed by ${overflow}px`);
});
