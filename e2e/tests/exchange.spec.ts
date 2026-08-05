import { test, expect } from "@playwright/test";

// POST /ui/exchange — the fail-closed error path, driven through a real form
// submission. With no configured Supabase project (the e2e boot has zero), any
// token fails verification and the server returns the static error fragment.
test.describe("token exchange submission", () => {
  test("an invalid token renders the unauthorized fragment", async ({ page }) => {
    await page.goto("/ui");
    await page.locator('textarea[name="access_token"]').fill("eyJ-not-a-real-supabase-token");
    await page.getByRole("button", { name: "Exchange" }).click();

    await expect(page).toHaveURL(/\/ui\/exchange$/);
    await expect(page.locator("h2.err")).toHaveText("✗ unauthorized");
    await expect(
      page.getByText("The token could not be verified against any configured Supabase project."),
    ).toBeVisible();
    // Fail-closed path must not leak a minted token or claims table.
    await expect(page.locator("h2.ok")).toHaveCount(0);
    await expect(page.locator("pre code")).toHaveCount(0);
  });

  test("the submitted token is never reflected into the response (no XSS surface)", async ({
    page,
  }) => {
    const marker = "zz-reflect-probe-<b>x</b>-zz";
    await page.goto("/ui");
    await page.locator('textarea[name="access_token"]').fill(marker);
    await page.getByRole("button", { name: "Exchange" }).click();

    await expect(page.locator("h2.err")).toBeVisible();
    // Neither the raw marker nor an injected element may appear in the document.
    const body = await page.content();
    expect(body).not.toContain("zz-reflect-probe");
    await expect(page.locator("body b")).toHaveCount(0);
  });
});
