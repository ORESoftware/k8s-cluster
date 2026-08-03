// Real-browser session-lifecycle regression coverage for the customer portal.
// Boots the same Rust/Postgres/stub-authority stack as the main E2E and proves
// logout cannot be forged, borrowed cross-origin, or leave residual credentials.
import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import {
  assertNoBrowserErrors,
  captureBrowserEvidence,
  observeBrowserErrors,
} from "./customer-browser-evidence.mjs";
import {
  chromeExecutablePath,
  CUSTOMER,
  startCustomer,
  STUB_OTP_CODE,
  unavailableReason,
} from "./customer-browser-harness.mjs";

const SESSION_COOKIE_SUFFIX = "fiducia_customer_session";
const LOGIN_CSRF_COOKIE_SUFFIX = "fiducia_customer_login_csrf";
const MFA_PENDING_COOKIE_SUFFIX = "fiducia_customer_mfa_pending";

function cookieBySuffix(cookies, suffix) {
  return cookies.find((cookie) => cookie.name.endsWith(suffix));
}

function pathname(url) {
  return new URL(url).pathname;
}

test(
  "playwright proves customer login and logout clear the complete browser credential set",
  { timeout: 180_000 },
  async (t) => {
    const unavailable = unavailableReason();
    if (unavailable) {
      t.skip(unavailable);
      return;
    }

    const server = await startCustomer();
    let browser;
    let page;
    let browserErrors = [];

    t.after(async () => {
      await captureBrowserEvidence(
        "playwright-session-lifecycle",
        page,
        browserErrors,
      );
      await page?.close().catch(() => {});
      await browser?.close().catch(() => {});
      await server.stop();
    });

    browser = await chromium.launch({
      executablePath: chromeExecutablePath(),
      headless: true,
    });
    page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
    browserErrors = observeBrowserErrors(page);

    // A protected navigation with no credential must end on the login page and
    // must not manufacture an application session while redirecting.
    await page.goto(`${server.url}/app`, { waitUntil: "networkidle" });
    assert.equal(pathname(page.url()), "/login");
    let cookies = await page.context().cookies(server.url);
    assert.equal(cookieBySuffix(cookies, SESSION_COOKIE_SUFFIX), undefined);

    // The unauthenticated form is bound to a short-lived, HttpOnly, Strict
    // login-CSRF nonce. This is the only credential expected before login.
    const preLoginCsrf = cookieBySuffix(cookies, LOGIN_CSRF_COOKIE_SUFFIX);
    assert.ok(preLoginCsrf, "login page must issue its CSRF nonce cookie");
    assert.equal(preLoginCsrf.httpOnly, true);
    assert.equal(preLoginCsrf.sameSite, "Strict");
    assert.equal(cookieBySuffix(cookies, MFA_PENDING_COOKIE_SUFFIX), undefined);

    await page.fill("#magic-email", CUSTOMER.email);
    await page.getByRole("button", { name: "Email me a link" }).click();
    await page.getByText("Check your email").first().waitFor();
    await page.fill("#otp-code", STUB_OTP_CODE);
    await page.getByRole("button", { name: "Verify & continue" }).click();
    await page.getByText("Fiducia Customer Portal").first().waitFor();

    // Successful login rotates to one application cookie and removes all
    // pre-session credentials. A pending aal1 bearer must never survive into an
    // authenticated browser, even when the account did not require step-up.
    cookies = await page.context().cookies(server.url);
    const session = cookieBySuffix(cookies, SESSION_COOKIE_SUFFIX);
    assert.ok(session, "successful OTP login must issue a customer session");
    assert.equal(session.httpOnly, true);
    assert.equal(session.sameSite, "Strict");
    assert.equal(cookieBySuffix(cookies, LOGIN_CSRF_COOKIE_SUFFIX), undefined);
    assert.equal(cookieBySuffix(cookies, MFA_PENDING_COOKIE_SUFFIX), undefined);

    await page.goto(`${server.url}/app`, { waitUntil: "networkidle" });
    assert.equal(pathname(page.url()), "/app");
    await page.getByText("Dashboard").first().waitFor();

    const logoutForm = page.locator('form[action="/logout"]').first();
    await logoutForm.waitFor({ state: "attached" });
    const validCsrf = await logoutForm
      .locator('input[name="csrf_token"]')
      .inputValue();
    assert.ok(validCsrf, "logout form must carry the session-bound CSRF token");

    // An attacker cannot sign the user out with an ambient Strict session: a
    // same-origin request with a forged token and a foreign-Origin request with
    // the genuine token are both rejected before cookie clearing.
    const forged = await page.request.post(`${server.url}/logout`, {
      headers: { origin: server.url },
      form: { csrf_token: "forged" },
      maxRedirects: 0,
    });
    assert.equal(forged.status(), 403);
    assert.equal((await forged.json()).error, "customer_request_rejected");

    const crossOrigin = await page.request.post(`${server.url}/logout`, {
      headers: { origin: "https://attacker.example" },
      form: { csrf_token: validCsrf },
      maxRedirects: 0,
    });
    assert.equal(crossOrigin.status(), 403);
    assert.equal(
      (await crossOrigin.json()).error,
      "customer_request_rejected",
    );

    cookies = await page.context().cookies(server.url);
    assert.ok(
      cookieBySuffix(cookies, SESSION_COOKIE_SUFFIX),
      "rejected logout attempts must not clear the valid session",
    );
    const stillAuthenticated = await page.request.get(`${server.url}/app`, {
      maxRedirects: 0,
    });
    assert.equal(stillAuthenticated.status(), 200);

    // The genuine same-origin logout clears every customer auth cookie in one
    // response and redirects to the login surface. Inspect the raw Set-Cookie
    // headers as well as the resulting browser jar so a partial clear cannot
    // pass by accident.
    const logout = await page.request.post(`${server.url}/logout`, {
      headers: { origin: server.url },
      form: { csrf_token: validCsrf },
      maxRedirects: 0,
    });
    assert.equal(logout.status(), 303);
    assert.equal(logout.headers().location, "/login");

    const cleared = logout
      .headersArray()
      .filter(({ name }) => name.toLowerCase() === "set-cookie")
      .map(({ value }) => value);
    for (const suffix of [
      SESSION_COOKIE_SUFFIX,
      MFA_PENDING_COOKIE_SUFFIX,
      LOGIN_CSRF_COOKIE_SUFFIX,
    ]) {
      const header = cleared.find((value) => value.includes(suffix));
      assert.ok(header, `logout must clear ${suffix}`);
      assert.match(header, /Max-Age=0/i);
      assert.match(header, /HttpOnly/i);
      assert.match(header, /SameSite=Strict/i);
    }

    cookies = await page.context().cookies(server.url);
    assert.equal(cookieBySuffix(cookies, SESSION_COOKIE_SUFFIX), undefined);
    assert.equal(cookieBySuffix(cookies, MFA_PENDING_COOKIE_SUFFIX), undefined);
    assert.equal(cookieBySuffix(cookies, LOGIN_CSRF_COOKIE_SUFFIX), undefined);

    await page.goto(`${server.url}/app`, { waitUntil: "networkidle" });
    assert.equal(pathname(page.url()), "/login");
    await page.getByText("Sign in to Fiducia").first().waitFor();

    assertNoBrowserErrors(browserErrors);
  },
);
