// Security-header contract for dd-fabrication-web-server (see ../../README.md).
//
// Mirrors playwright/tests/security-headers.spec.mjs so a drift shows up in both
// frameworks identically. The CSP-enforcement assertion is the reason these run
// a real browser: a header assertion cannot tell a working policy from one with
// a typo'd directive or a stray 'unsafe-inline', because both look identical as
// a string. Only an engine that applies the policy can.
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import puppeteer from 'puppeteer';

const baseUrl = (process.env.DD_FAB_E2E_BASE_URL ?? 'http://127.0.0.1:8115').replace(/\/+$/, '');

/** Headers every response must carry, with the values that make them useful. */
const REQUIRED = {
  'x-content-type-options': 'nosniff',
  'x-frame-options': 'DENY',
  'referrer-policy': 'strict-origin-when-cross-origin',
  'cache-control': 'private, no-store',
};

// `/` is authenticated, so anonymously it is an error response — deliberately
// included. `/healthz` is public. Between them they cover both a handler-made
// and a layer-made response.
const SURFACES = ['/healthz', '/'];

let browser;
let page;

before(async () => {
  browser = await puppeteer.launch({ headless: true });
  page = await browser.newPage();
});

after(async () => {
  if (browser) await browser.close();
});

for (const path of SURFACES) {
  test(`${path} carries every security header`, async () => {
    const response = await page.goto(`${baseUrl}${path}`);
    const headers = response.headers();

    for (const [name, value] of Object.entries(REQUIRED)) {
      assert.equal(headers[name], value, `${path} is missing or wrong: ${name}`);
    }

    const csp = headers['content-security-policy'];
    assert.ok(csp, `${path} is missing content-security-policy`);
    // The hashes are what let the policy stay strict while the page keeps its
    // one inline <script>. If 'unsafe-inline' ever appears they become
    // decorative and injected script runs again.
    assert.ok(!csp.includes("'unsafe-inline'"), "CSP must not allow 'unsafe-inline'");
    assert.ok(!csp.includes("'unsafe-eval'"), "CSP must not allow 'unsafe-eval'");
    assert.ok(csp.includes("frame-ancestors 'none'"), 'CSP must forbid framing');
    assert.ok(csp.includes("object-src 'none'"), 'CSP must forbid plugins');
    assert.ok(csp.includes("base-uri 'none'"), 'CSP must pin the base URI');
  });
}

test('the content security policy is enforced by the browser', async () => {
  // Any document from this origin will do: the policy is attached by an
  // outermost layer, so it is on the anonymous error response too.
  const response = await page.goto(`${baseUrl}/`);
  assert.ok(response, 'the server must answer /');

  const outcome = await page.evaluate(
    () =>
      new Promise((resolve) => {
        let violated = false;
        document.addEventListener(
          'securitypolicyviolation',
          (event) => {
            if (event.violatedDirective.startsWith('script-src')) violated = true;
          },
          { once: true },
        );
        // Build the element by hand rather than via addScriptTag: the automation
        // helpers can be CSP-exempt, which would make this pass vacuously.
        const script = document.createElement('script');
        script.textContent = 'window.__cspInlineRan = true;';
        document.documentElement.appendChild(script);
        // One turn of the event loop is enough; inline script is synchronous.
        setTimeout(() => resolve({ ran: window.__cspInlineRan === true, violated }), 50);
      }),
  );

  assert.equal(outcome.ran, false, 'injected inline script executed — the CSP is not blocking it');
  assert.equal(outcome.violated, true, 'no script-src violation was reported');
});
