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
    });
  }

  test("the UI ships no JavaScript", async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on("console", (m) => m.type() === "error" && consoleErrors.push(m.text()));
    page.on("pageerror", (e) => consoleErrors.push(String(e)));

    await page.goto("/ui");
    // No <script> elements anywhere, and the inert #result stays empty — the
    // page cannot execute code, so the CSP's script restrictions can't break it.
    await expect(page.locator("script")).toHaveCount(0);
    await expect(page.locator("#result")).toBeEmpty();
    expect(consoleErrors).toEqual([]);
  });

  test("cannot be framed (clickjacking defense is enforceable)", async ({ page }) => {
    // frame-ancestors 'none' + XFO DENY: assert both directives are present so a
    // conforming browser refuses to embed the page.
    const response = await page.goto("/");
    const h = response!.headers();
    expect(h["x-frame-options"]).toBe("DENY");
    expect(h["content-security-policy"]).toContain("frame-ancestors 'none'");
  });
});
