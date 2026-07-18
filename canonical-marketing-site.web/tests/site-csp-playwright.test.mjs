import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

const marketingCsp = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data:",
  "base-uri 'self'",
  "form-action 'self'",
  "frame-ancestors 'none'",
  "object-src 'none'",
].join("; ");

test("playwright: production assets execute under the Rust marketing CSP", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage({ viewport: { height: 812, width: 375 } });
  const cspViolations = [];
  page.on("console", (message) => {
    if (/content security policy|refused to (?:load|execute)/i.test(message.text())) {
      cspViolations.push(message.text());
    }
  });
  await page.addInitScript(() => {
    window.__canonicalCspViolations = [];
    document.addEventListener("securitypolicyviolation", (event) => {
      window.__canonicalCspViolations.push({
        blockedUri: event.blockedURI,
        directive: event.effectiveDirective,
      });
    });
  });

  await page.route("**/*", async (route) => {
    const response = await route.fetch();
    await route.fulfill({
      response,
      headers: {
        ...response.headers(),
        "content-security-policy": marketingCsp,
      },
    });
  });

  await page.goto(`${server.url}/`, { waitUntil: "networkidle" });

  await page.evaluate(() => window.scrollTo(0, 200));
  await page.waitForFunction(
    () => document.getElementById("main-nav")?.style.borderBottomColor !== "",
  );

  await page.locator("#nav-toggle").click();
  const mobileMenuOpened = await page.locator("#nav-links").evaluate((node) =>
    node.classList.contains("nav__links--open"),
  );
  assert.equal(mobileMenuOpened, true);
  assert.deepEqual(cspViolations, []);
  assert.deepEqual(await page.evaluate(() => window.__canonicalCspViolations), []);
});
