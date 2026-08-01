import { test, expect } from "@playwright/test";

// GET /docs/api (and its /api/docs alias) — the static human-readable API page,
// plus the machine-readable OpenAPI document it links to.
test.describe("API docs", () => {
  test("/docs/api renders and lists the auth endpoints", async ({ page }) => {
    const response = await page.goto("/docs/api");
    expect(response?.status()).toBe(200);
    await expect(page).toHaveTitle("shared-auth API");
    await expect(page.locator("h1")).toHaveText("shared-auth API");

    for (const endpoint of ["POST /auth/login", "POST /auth/exchange", "GET /.well-known/jwks.json"]) {
      await expect(page.locator("code", { hasText: endpoint })).toBeVisible();
    }
    await expect(page.locator('a[href="/api/docs.json"]')).toBeVisible();
  });

  test("/api/docs is an alias of the same page", async ({ page }) => {
    await page.goto("/api/docs");
    await expect(page.locator("h1")).toHaveText("shared-auth API");
  });

  test("/api/docs.json is a valid OpenAPI 3.1 document", async ({ request }) => {
    const res = await request.get("/api/docs.json");
    expect(res.status()).toBe(200);
    expect(res.headers()["content-type"]).toContain("application/json");
    const doc = await res.json();
    expect(doc.openapi).toBe("3.1.0");
    expect(doc.info.title).toBe("shared-auth API");
    expect(Object.keys(doc.paths).length).toBeGreaterThan(0);
  });
});
