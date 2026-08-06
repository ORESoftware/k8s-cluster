import { test, expect } from "@playwright/test";

// GET / — the status landing page. Rendered by views::page("status", …).
test.describe("landing /", () => {
  test("renders the status page shell", async ({ page }) => {
    const response = await page.goto("/");
    expect(response?.status()).toBe(200);
    expect(response?.headers()["content-type"]).toContain("text/html");

    await expect(page).toHaveTitle("status · shared-auth");
    await expect(page.locator("h1")).toHaveText("shared-auth-server");
    await expect(page.getByRole("heading", { name: "status" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "endpoints" })).toBeVisible();
  });

  test("shows the issuer and a numeric Supabase project count", async ({ page }) => {
    await page.goto("/");
    // The status table lists issuer / supabase projects / Postgres authority.
    await expect(page.getByText("issuer")).toBeVisible();
    await expect(page.locator("td", { hasText: "supabase projects" })).toBeVisible();
    // The count cell is a bare integer; assert one is present next to the label.
    const rowText = await page
      .locator("tr", { has: page.locator("td", { hasText: "supabase projects" }) })
      .innerText();
    expect(rowText).toMatch(/supabase projects\s+\d+/);
  });

  test("links to the token-exchange helper", async ({ page }) => {
    await page.goto("/");
    const link = page.locator('a[href="ui"]');
    await expect(link).toHaveText("→ token exchange helper");
    await link.click();
    await expect(page).toHaveURL(/\/ui$/);
    await expect(page.locator("h1")).toHaveText("token exchange");
  });

  test("advertises the JSON auth endpoints", async ({ page }) => {
    await page.goto("/");
    for (const path of ["POST /auth/exchange", "POST /auth/introspect"]) {
      await expect(page.locator("code", { hasText: path })).toBeVisible();
    }
  });
});
