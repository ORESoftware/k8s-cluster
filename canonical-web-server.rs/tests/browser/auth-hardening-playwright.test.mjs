import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startServer } from "./app-browser-harness.mjs";

const TEST_AUTH_EMAIL = "browser-e2e@canonical.invalid";
const TEST_AUTH_PASSWORD = "browser-e2e-only";
const SESSION_COOKIE = "canonical_session";
const STORAGE_SENTINEL = "canonical-e2e-private-state";
const CACHE_SENTINEL = "canonical-e2e-private-cache";

async function waitUntil(predicate, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for ${description}`);
}

test("playwright rejects hostile login origins and clears origin data on logout", async (t) => {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  const loginPage = await page.goto(`${server.url}/login`, { waitUntil: "networkidle" });
  assert.ok(loginPage);
  assert.equal(loginPage.status(), 200);
  const loginHeaders = loginPage.headers();
  assert.equal(loginHeaders["cache-control"], "no-store");
  assert.equal(loginHeaders["strict-transport-security"], "max-age=31536000");
  assert.equal(loginHeaders["cross-origin-opener-policy"], "same-origin");
  assert.match(loginHeaders["permissions-policy"], /camera=\(\)/);
  assert.match(
    loginHeaders["x-request-id"],
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );

  const csp = loginHeaders["content-security-policy"];
  assert.match(csp, /script-src 'self';/);
  assert.match(csp, /frame-ancestors 'none'/);
  assert.doesNotMatch(csp, /script-src[^;]*'unsafe-inline'/);

  // Prove that Chromium enforces the application CSP, not merely that the
  // response contains a plausible header string.
  await page.evaluate(() => {
    const script = document.createElement("script");
    script.textContent = "window.__canonicalInlineScriptExecuted = true";
    document.head.append(script);
  });
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(() => window.__canonicalInlineScriptExecuted), undefined);

  const csrf = await page.locator('input[name="csrf"]').getAttribute("value");
  assert.ok(csrf?.length > 20);

  // The hostile request has the genuine double-submit cookie and form token;
  // only the Origin is wrong. It must still fail before authentication or
  // session creation.
  const hostileLogin = await page.request.post(`${server.url}/auth/login`, {
    headers: { origin: "https://attacker.invalid" },
    form: {
      email: TEST_AUTH_EMAIL,
      password: TEST_AUTH_PASSWORD,
      csrf,
    },
  });
  assert.equal(hostileLogin.status(), 403);
  assert.equal(
    (await context.cookies(server.url)).some((cookie) => cookie.name === SESSION_COOKIE),
    false,
  );

  await page.locator('input[name="email"]').fill(TEST_AUTH_EMAIL);
  await page.locator('input[name="password"]').fill(TEST_AUTH_PASSWORD);
  const loginResponsePromise = page.waitForResponse(
    (response) =>
      response.url() === `${server.url}/auth/login` &&
      response.request().method() === "POST",
  );
  await page.locator('form[action="/auth/login"] button[type="submit"]').click();
  assert.equal((await loginResponsePromise).status(), 200);
  await page.waitForURL(`${server.url}/app`);

  const sessionCookie = (await context.cookies(server.url)).find(
    (cookie) => cookie.name === SESSION_COOKIE,
  );
  assert.ok(sessionCookie);
  assert.equal(sessionCookie.httpOnly, true);
  assert.equal(sessionCookie.sameSite, "Lax");

  // Model sensitive offline state from the real application: one synchronous
  // browser-storage value and one Cache Storage entry on the customer origin.
  await page.evaluate(async ({ storageSentinel, cacheSentinel }) => {
    localStorage.setItem(storageSentinel, "customer-private-value");
    const cache = await caches.open(cacheSentinel);
    await cache.put("/__canonical_e2e_private", new Response("customer-private-value"));
  }, { storageSentinel: STORAGE_SENTINEL, cacheSentinel: CACHE_SENTINEL });

  const logoutResponsePromise = page.waitForResponse(
    (response) =>
      response.url() === `${server.url}/auth/logout` &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  const logoutResponse = await logoutResponsePromise;
  assert.equal(logoutResponse.status(), 303);
  // Playwright's compact `headers()` API intentionally omits security-related
  // response headers. Read the complete wire-visible set for this boundary.
  const logoutHeaders = await logoutResponse.allHeaders();
  assert.equal(
    logoutHeaders["clear-site-data"],
    '"cache", "cookies", "storage"',
  );
  await page.waitForURL(`${server.url}/login`);

  await waitUntil(async () => {
    const state = await page.evaluate(async ({ storageSentinel, cacheSentinel }) => ({
      local: localStorage.getItem(storageSentinel),
      cachePresent: (await caches.keys()).includes(cacheSentinel),
    }), { storageSentinel: STORAGE_SENTINEL, cacheSentinel: CACHE_SENTINEL });
    return state.local === null && state.cachePresent === false;
  }, "Clear-Site-Data to remove browser storage and cache state");

  assert.equal(
    (await context.cookies(server.url)).some((cookie) => cookie.name === SESSION_COOKIE),
    false,
  );
  assert.deepEqual(pageErrors, []);
});
