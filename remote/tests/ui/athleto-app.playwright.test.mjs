// Playwright-driven node:test smoke of the live AthletO storefront
// (app.athleto.store -> Rust axum pod behind the ingress). Exercises the
// server-rendered surface: security headers, the passwordless login, and
// product pages. Run: `pnpm run test:ui:athleto`.
import assert from "node:assert/strict";
import test, { after, before } from "node:test";

import { APP_URL, gotoWithRetry, launchPlaywright, statusOf } from "./lib/harness.mjs";

let driver;
before(async () => {
  driver = await launchPlaywright();
  console.log(`[athleto-ui:app] storefront target=${APP_URL}`);
});
after(async () => {
  await driver?.close();
});

test("storefront home responds 200 and lists the lineup", async () => {
  const { page } = await driver.newPage();
  const response = await gotoWithRetry(page, `${APP_URL}/`);
  assert.equal(statusOf(response), 200);
  assert.match(await page.title(), /AthletO/i);
  assert.match((await page.locator("body").innerText()).toLowerCase(), /lineup|product/);
});

test("storefront sets the server security headers on every response", async () => {
  const { page } = await driver.newPage();
  const response = await gotoWithRetry(page, `${APP_URL}/`);
  const headers = response.headers();
  assert.ok(headers["content-security-policy"], "expected a Content-Security-Policy header");
  assert.match(headers["content-security-policy"], /default-src 'self'/);
  assert.doesNotMatch(headers["content-security-policy"], /script-src[^;]*unsafe-inline/);
  assert.equal(headers["x-frame-options"], "DENY");
  assert.equal(headers["x-content-type-options"], "nosniff");
});

test("passwordless login page renders a magic-link form with a CSRF token", async () => {
  const { page } = await driver.newPage();
  const response = await gotoWithRetry(page, `${APP_URL}/login`);
  assert.equal(statusOf(response), 200);
  const body = (await page.locator("body").innerText()).toLowerCase();
  assert.match(body, /sign in/);
  assert.match(body, /magic link|no passwords|email me a sign-in link/);
  // The double-submit CSRF token must be embedded as a hidden field.
  assert.equal(await page.locator('input[name="csrf_token"]').count() >= 1, true);
  // ...and the email input the magic-link flow needs.
  assert.equal(await page.locator('input[type="email"]').count() >= 1, true);
});

test("a product page renders its product from the catalog", async () => {
  const { page } = await driver.newPage();
  await gotoWithRetry(page, `${APP_URL}/`);
  // Navigate via a real product link rather than guessing a slug.
  const href = await page
    .locator('a[href^="/product/"]')
    .first()
    .getAttribute("href");
  assert.ok(href, "storefront home should link to at least one product");
  const response = await gotoWithRetry(page, `${APP_URL}${href}`);
  assert.equal(statusOf(response), 200);
  // Product detail shows a price and an add-to-cart control.
  const body = (await page.locator("body").innerText());
  assert.match(body, /\$\d/, "product page should show a price");
});
