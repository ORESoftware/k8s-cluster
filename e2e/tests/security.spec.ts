import { test, expect } from "@playwright/test";

// Security posture observable through a real browser: the response headers the
// server sets on every route, and the fact that the UI is entirely script-free
// (so the strict `default-src 'none'` CSP never breaks it).
test.describe("security headers & script-free UI", () => {
  const routes = ["/", "/ui"];

  for (const route of routes) {
    test(`${route} carries the hardening headers`, async ({ page }) => {
      const response = await page.goto(route);
      const h = response!.headers();
      expect(h["content-security-policy"]).toContain("default-src 'none'");
      expect(h["content-security-policy"]).toContain("frame-ancestors 'none'");
      expect(h["content-security-policy"]).toContain("form-action 'self'");
      expect(h["x-frame-options"]).toBe("DENY");
      expect(h["x-content-type-options"]).toBe("nosniff");
      expect(h["referrer-policy"]).toBe("no-referrer");
      expect(h["strict-transport-security"]).toContain("max-age=");
      expect(h["cache-control"]).toBe("no-store");
      expect(h["permissions-policy"]).toContain("camera=()");
      expect(h["permissions-policy"]).toContain("microphone=()");
      expect(h["cross-origin-opener-policy"]).toBe("same-origin");
    });
  }

  test("public JWKS caching survives the global no-store policy", async ({ request }) => {
    const response = await request.get("/.well-known/jwks.json");
    expect(response.ok()).toBeTruthy();
    const cacheControl = response.headers()["cache-control"] ?? "";
    expect(cacheControl).toContain("public");
    expect(cacheControl).toContain("max-age=300");
    expect(cacheControl).not.toContain("no-store");
  });

  test("the UI ships no JavaScript", async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on("console", (m) => m.type() === "error" && consoleErrors.push(m.text()));
    page.on("pageerror", (e) => consoleErrors.push(String(e)));

    await page.goto("/ui");
    await expect(page.locator("script")).toHaveCount(0);
    await expect(page.locator("#result")).toBeEmpty();
    expect(consoleErrors).toEqual([]);
  });

  test("cannot be framed (clickjacking defense is enforceable)", async ({ page }) => {
    const response = await page.goto("/");
    const h = response!.headers();
    expect(h["x-frame-options"]).toBe("DENY");
    expect(h["content-security-policy"]).toContain("frame-ancestors 'none'");
  });
});
