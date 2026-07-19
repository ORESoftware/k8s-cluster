// Playwright e2e: the daedalus-web-server Supabase auth gate.
//
// Env-gated. Set DAEDALUS_WEB_BASE_URL to a running daedalus-web-server to run;
// unset, every test is skipped so base CI stays green without a live stack.
//   DAEDALUS_WEB_BASE_URL=https://app.daedalus-fab.com \
//   DAEDALUS_WEB_TOKEN=<supabase-access-token> \
//   npx playwright test
//
// The token is OPTIONAL: without it, only the unauthenticated assertions run
// (health, and that protected routes refuse anonymous access). With an
// allow-listed token, the authenticated page is asserted to render.

import { test, expect } from "@playwright/test";

const BASE = process.env.DAEDALUS_WEB_BASE_URL;
const TOKEN = process.env.DAEDALUS_WEB_TOKEN;

test.skip(!BASE, "set DAEDALUS_WEB_BASE_URL to run e2e against a live web server");

test("health endpoint is reachable and unauthenticated", async ({ request }) => {
  const response = await request.get(`${BASE}/health`);
  expect(response.ok()).toBeTruthy();
});

test("the plans page refuses anonymous access", async ({ request }) => {
  // No Authorization header. The server must NOT return the plans page: either
  // 401 (auth configured, token missing) or 503 (auth not configured). A 200
  // here would mean the auth gate is not being enforced.
  const response = await request.get(`${BASE}/`, {
    headers: { accept: "text/html" },
    failOnStatusCode: false,
  });
  expect(response.status(), "anonymous access must be rejected").not.toBe(200);
  expect([401, 503]).toContain(response.status());
});

test("a bogus bearer token is rejected", async ({ request }) => {
  const response = await request.get(`${BASE}/`, {
    headers: { authorization: "Bearer not-a-real-token", accept: "text/html" },
    failOnStatusCode: false,
  });
  // A forged token must never authenticate. 401 (rejected) or 503 (auth off).
  expect(response.status()).not.toBe(200);
});

test.describe("with an allow-listed token", () => {
  test.skip(!TOKEN, "set DAEDALUS_WEB_TOKEN to run authenticated assertions");

  test("the plans page renders for an authorized operator", async ({ page }) => {
    await page.route("**/*", (route) => {
      const headers = { ...route.request().headers(), authorization: `Bearer ${TOKEN}` };
      route.continue({ headers });
    });
    const response = await page.goto(`${BASE}/`, { waitUntil: "domcontentloaded" });
    expect(response?.status()).toBe(200);
    // The landing view renders this heading (see daedalus-web-server views.rs).
    await expect(page.getByRole("heading", { name: /fabrication plans/i })).toBeVisible();
    // The htmx bundle must be served from same-origin /assets, not a CDN.
    const scripts = await page.locator("script[src]").evaluateAll((els) =>
      els.map((e) => e.getAttribute("src")),
    );
    expect(scripts.some((src) => src?.startsWith("/assets/htmx-"))).toBeTruthy();
  });
});
