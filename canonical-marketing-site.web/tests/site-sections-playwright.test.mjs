import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

// Second Playwright spec file (alongside site-playwright.test.mjs): covers the
// frameworks / differentiators / social-proof sections and the page-level
// concerns (SEO metadata, single-h1 accessibility, mobile layout) that the
// smoke spec does not.

async function open(t, viewport = { height: 900, width: 1440 }) {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    // --no-sandbox: CI runners drive Chrome as root, where the sandbox refuses
    // to start; harmless locally.
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage({ viewport });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.goto(`${server.url}/`, { waitUntil: "networkidle" });
  return { page, pageErrors };
}

test("playwright: frameworks grid lists all six audited frameworks with tiers", async (t) => {
  const { page, pageErrors } = await open(t);

  const frameworks = await page.locator("#frameworks .frameworks__item").evaluateAll((items) =>
    items.map((item) => ({
      name: item.querySelector("h4")?.textContent?.trim(),
      tier: item.querySelector("p")?.textContent?.trim(),
    })),
  );
  assert.deepEqual(frameworks, [
    { name: "SOC 2", tier: "Type I & II" },
    { name: "FedRAMP", tier: "Li/Mod/High" },
    { name: "HIPAA", tier: "Security & Privacy" },
    { name: "ISO 27001", tier: "Certification" },
    { name: "PCI DSS", tier: "Level 1–4" },
    { name: "GDPR", tier: "Data Protection" },
  ]);
  assert.deepEqual(pageErrors, []);
});

test("playwright: the 'why' section presents the four differentiators", async (t) => {
  const { page, pageErrors } = await open(t);

  const items = await page.locator("#about .why__item h4").allInnerTexts();
  assert.deepEqual(
    items.map((text) => text.trim()),
    ["Startup-First Pricing", "Technology-Powered", "Experienced CPAs", "Equity-Friendly Model"],
  );
  assert.deepEqual(pageErrors, []);
});

test("playwright: testimonial and contact CTA render the social proof + conversion path", async (t) => {
  const { page, pageErrors } = await open(t);

  await page
    .locator(".testimonial__text")
    .filter({ hasText: /SOC 2 Type II in 6 weeks/ })
    .first()
    .waitFor({ state: "visible" });
  assert.match(await page.locator(".testimonial__name").innerText(), /James Rodriguez/);
  assert.match(await page.locator(".testimonial__role").innerText(), /CTO/);

  const contact = page.locator("#contact");
  await contact.waitFor({ state: "visible" });
  await contact
    .getByRole("link", { name: /Schedule a Free Consultation/ })
    .waitFor({ state: "visible" });
  assert.equal(
    await page.locator('#cta-contact-btn').getAttribute("href"),
    "mailto:compliance@canonical.cloud",
  );
  assert.equal(await page.locator("#cta-services-btn").getAttribute("href"), "#services");
  assert.deepEqual(pageErrors, []);
});

test("playwright: page ships SEO/social metadata, a single h1, and survives a mobile viewport", async (t) => {
  const { page, pageErrors } = await open(t, { height: 812, width: 375 });

  // SEO + Open Graph metadata (server-rendered in <head>).
  const meta = async (selector) =>
    page.locator(selector).getAttribute("content");
  assert.match(await meta('meta[name="description"]'), /SOC 2, FedRAMP and HIPAA compliance audits/);
  assert.match(await meta('meta[property="og:title"]'), /Compliance Audits/);
  assert.ok((await meta('meta[property="og:description"]'))?.length > 0);

  // Accessibility: exactly one <h1> on the page.
  assert.equal(await page.locator("h1").count(), 1);

  // Mobile smoke: the brand and primary hero CTA are still present at 375px,
  // and the document does not overflow horizontally.
  await page.locator(".nav__logo-text").first().waitFor({ state: "attached" });
  await page.locator("#hero-cta-primary").first().waitFor({ state: "visible" });
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth <= window.innerWidth + 1,
  );
  assert.ok(overflow, "page should not scroll horizontally on a 375px viewport");
  assert.deepEqual(pageErrors, []);
});
