import { test, expect } from "@playwright/test";

// Browser end-to-end tests for the t2v-web MASH dashboard. These complement the
// Rust `web_smoke` integration tests: they run the page in a REAL browser, so
// they prove things the router tests cannot — that the vendored htmx script
// actually loads and executes under the strict `script-src 'self'` CSP, that
// the live-stats websocket connects, and that navigation works.

test("dashboard loads with the hero and live stat cards", async ({ page }) => {
  const response = await page.goto("/");
  expect(response?.status()).toBe(200);
  await expect(page).toHaveTitle(/t2v/i);
  await expect(page.locator(".hero h1")).toContainText("Voice");
  // Four live metric cards.
  await expect(page.locator("#stat-transcriptions")).toBeVisible();
  await expect(page.locator("#stat-translations")).toBeVisible();
  await expect(page.locator("#stat-syntheses")).toBeVisible();
  await expect(page.locator("#stat-vapi")).toBeVisible();
});

test("response carries the hardening headers", async ({ page }) => {
  const response = await page.goto("/");
  const headers = response!.headers();
  expect(headers["content-security-policy"]).toContain("default-src 'self'");
  expect(headers["content-security-policy"]).toContain("script-src 'self'");
  expect(headers["content-security-policy"]).not.toContain("unpkg");
  expect(headers["x-content-type-options"]).toBe("nosniff");
  expect(headers["x-frame-options"]).toBe("DENY");
  expect(headers["referrer-policy"]).toBe("no-referrer");
});

test("vendored htmx loads and executes under the strict CSP (no CDN)", async ({ page }) => {
  const requested: string[] = [];
  page.on("request", (r) => requested.push(r.url()));
  const cspViolations: string[] = [];
  page.on("console", (m) => {
    if (m.text().includes("Content Security Policy")) cspViolations.push(m.text());
  });

  await page.goto("/", { waitUntil: "networkidle" });

  // No third-party host was contacted — htmx is same-origin.
  expect(requested.some((u) => u.includes("unpkg.com") || u.includes("cdn"))).toBe(false);
  expect(requested.some((u) => u.endsWith("/assets/htmx.min.js"))).toBe(true);

  // The vendored script actually ran under `script-src 'self'`: htmx's global
  // is defined. If the CSP had blocked it, this would be undefined.
  const htmxVersion = await page.evaluate(() => (window as any).htmx?.version);
  expect(htmxVersion, "htmx global should be defined (script executed)").toBeTruthy();
  expect(cspViolations, `unexpected CSP violations: ${cspViolations.join("; ")}`).toHaveLength(0);
});

test("live-stats websocket connects", async ({ page }) => {
  const wsPromise = page.waitForEvent("websocket", { timeout: 8_000 });
  await page.goto("/");
  const ws = await wsPromise;
  // htmx ws extension connects to the same-origin /ws/stats endpoint.
  expect(ws.url()).toContain("/ws/stats");
});

test("assets are served self-hosted with correct content types", async ({ request }) => {
  const js = await request.get("/assets/htmx.min.js");
  expect(js.status()).toBe(200);
  expect(js.headers()["content-type"]).toContain("javascript");
  expect((await js.body()).length).toBeGreaterThan(10_000);

  const css = await request.get("/assets/app.css");
  expect(css.status()).toBe(200);
  expect(css.headers()["content-type"]).toContain("text/css");
});

test("navigation to translate and speak renders interactive forms", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "Translate" }).click();
  await expect(page).toHaveURL(/\/translate$/);
  await expect(page.locator('form[hx-post="/translate"]')).toBeVisible();
  await expect(page.locator('input[name="target_lang"]')).toBeVisible();

  await page.getByRole("link", { name: "Text to Speech" }).click();
  await expect(page).toHaveURL(/\/speak$/);
  await expect(page.locator('form[hx-post="/speak"]')).toBeVisible();
});

test("history page renders (empty state) without error", async ({ page }) => {
  const response = await page.goto("/history");
  expect(response?.status()).toBe(200);
  await expect(page.locator(".hero h1")).toContainText("History");
});
