// Security-header contract for dd-fabrication-web-server (see ../../README.md).
//
// These assertions are the reason this suite runs a real browser rather than
// curl. Two of them cannot be made any other way:
//
//   * `the content security policy is enforced by the browser` proves the
//     policy *works*, not merely that the header string is present. A CSP with
//     a typo'd directive, or one that silently ships 'unsafe-inline', still
//     looks correct in a header assertion and still lets injected script run.
//     Only an engine that parses and applies it can tell you the difference.
//   * the header assertions run against an *error* response, which is the
//     surface most likely to regress: it is produced by a layer, not a handler,
//     so it is invisible to any test that only exercises routes.
import { expect, test } from '@playwright/test';

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

for (const path of SURFACES) {
  test(`${path} carries every security header`, async ({ request }) => {
    const response = await request.get(path);
    const headers = response.headers();

    for (const [name, value] of Object.entries(REQUIRED)) {
      expect(headers[name], `${path} is missing ${name}`).toBe(value);
    }

    const csp = headers['content-security-policy'];
    expect(csp, `${path} is missing content-security-policy`).toBeTruthy();
    // The hashes are what let the policy stay strict while the page keeps its
    // one inline <script>. If 'unsafe-inline' ever appears they become
    // decorative and injected script runs again.
    expect(csp).not.toContain("'unsafe-inline'");
    expect(csp).not.toContain("'unsafe-eval'");
    expect(csp).toContain("frame-ancestors 'none'");
    expect(csp).toContain("object-src 'none'");
    expect(csp).toContain("base-uri 'none'");
  });
}

test('the content security policy is enforced by the browser', async ({ page }) => {
  // Any document from this origin will do: the policy is attached by an
  // outermost layer, so it is on the anonymous error response too.
  const response = await page.goto('/');
  expect(response, 'the server must answer /').not.toBeNull();

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

  expect(outcome.ran, 'injected inline script executed — the CSP is not blocking it').toBe(false);
  expect(outcome.violated, 'no script-src violation was reported').toBe(true);
});
