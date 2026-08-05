import { test, expect } from "@playwright/test";

// GET /ui — the token-exchange helper form. Rendered by views::sign_in().
test.describe("token-exchange helper /ui", () => {
  test("renders the exchange form", async ({ page }) => {
    const response = await page.goto("/ui");
    expect(response?.status()).toBe(200);
    await expect(page).toHaveTitle("token exchange · shared-auth");
    await expect(page.locator("h1")).toHaveText("token exchange");

    const form = page.locator('form[method="post"][action="ui/exchange"]');
    await expect(form).toBeVisible();

    const textarea = form.locator('textarea[name="access_token"]');
    await expect(textarea).toBeVisible();
    // The field is required — the server never has to defend an empty submit.
    await expect(textarea).toHaveAttribute("required", "");
    await expect(textarea).toHaveAttribute("placeholder", /Supabase access token/);

    await expect(form.getByRole("button", { name: "Exchange" })).toBeVisible();
  });

  test("the #result div is present but inert (no script populates it)", async ({ page }) => {
    await page.goto("/ui");
    // The form does a full-page native POST — #result is dead markup, never swapped.
    await expect(page.locator("#result")).toBeAttached();
    await expect(page.locator("#result")).toBeEmpty();
  });

  test("links back to the status page", async ({ page }) => {
    await page.goto("/ui");
    const back = page.locator('a[href="."]');
    await expect(back).toHaveText("← status");
    await back.click();
    await expect(page.locator("h1")).toHaveText("shared-auth-server");
  });

  test("required field blocks an empty submit (stays on /ui)", async ({ page }) => {
    await page.goto("/ui");
    await page.getByRole("button", { name: "Exchange" }).click();
    // Native form validation prevents navigation; still on /ui, field invalid.
    await expect(page).toHaveURL(/\/ui$/);
    const invalid = await page
      .locator('textarea[name="access_token"]')
      .evaluate((el: HTMLTextAreaElement) => !el.validity.valid);
    expect(invalid).toBe(true);
  });
});
