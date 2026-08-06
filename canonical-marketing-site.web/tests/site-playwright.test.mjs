import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

async function launchBrowser(t) {
  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    // --no-sandbox: CI runners drive Chrome as root, where the sandbox refuses
    // to start; harmless locally.
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());
  return browser;
}

test("playwright renders the canonical.cloud landing page", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());
  const browser = await launchBrowser(t);

  const page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto(`${server.url}/`, { waitUntil: "networkidle" });
  assert.equal(await page.title(), "SOC 2, FedRAMP & HIPAA Compliance Audits | canonical.cloud");

  // Hero <h1> — headline split across a <br>, so normalize whitespace.
  const hero = page.getByRole("heading", { level: 1 });
  await hero.waitFor({ state: "visible" });
  assert.match(
    (await hero.innerText()).replace(/\s+/g, " ").trim(),
    /Compliance Audits\s*Without the Overhead/,
  );

  // Nav: brand and the four section links.
  await page.locator(".nav__logo-text").filter({ hasText: "CANONICAL" }).first().waitFor({ state: "visible" });
  for (const label of ["Services", "Process", "Frameworks", "About"]) {
    await page.locator(".nav__link", { hasText: label }).first().waitFor({ state: "visible" });
  }

  // The four compliance service cards.
  for (const svc of [
    "SOC 2 Attestation",
    "FedRAMP Authorization",
    "HIPAA Compliance",
    "vCISO & IT Advisory",
  ]) {
    await page.getByRole("heading", { name: svc, exact: true }).waitFor({ state: "visible" });
  }

  // Primary contact CTA (mailto).
  const contact = page.locator('a[href="mailto:compliance@canonical.cloud"]');
  await contact.first().waitFor({ state: "visible" });

  // Footer copyright.
  await page.locator("footer").getByText(/canonical\.cloud\. All rights reserved/).waitFor({ state: "visible" });

  assert.deepEqual(pageErrors, []);
});

test("playwright keeps mobile navigation operable by keyboard and touch", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());
  const browser = await launchBrowser(t);

  const page = await browser.newPage({
    hasTouch: true,
    isMobile: true,
    viewport: { height: 844, width: 390 },
  });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto(`${server.url}/`, { waitUntil: "networkidle" });

  const toggle = page.locator("#nav-toggle");
  const links = page.locator("#nav-links");
  await toggle.waitFor({ state: "visible" });
  assert.equal(await toggle.getAttribute("type"), "button");
  assert.equal(await toggle.getAttribute("aria-controls"), "nav-links");
  assert.equal(await toggle.getAttribute("aria-expanded"), "false");
  assert.equal(await toggle.getAttribute("aria-label"), "Open navigation");
  assert.equal(await links.isVisible(), false);

  await toggle.focus();
  await page.keyboard.press("Enter");
  assert.equal(await toggle.getAttribute("aria-expanded"), "true");
  assert.equal(await toggle.getAttribute("aria-label"), "Close navigation");
  await links.waitFor({ state: "visible" });

  const firstLink = links.getByRole("link", { name: "Services", exact: true });
  const box = await firstLink.boundingBox();
  assert.ok(box && box.height >= 44, `mobile link target was ${box?.height ?? 0}px high`);

  await page.keyboard.press("Escape");
  assert.equal(await toggle.getAttribute("aria-expanded"), "false");
  assert.equal(await links.isVisible(), false);
  assert.equal(await page.evaluate(() => document.activeElement?.id), "nav-toggle");

  await toggle.click();
  await firstLink.click();
  await page.waitForURL(/#services$/);
  assert.equal(await toggle.getAttribute("aria-expanded"), "false");
  assert.equal(await links.isVisible(), false);
  await page.locator("#services").waitFor({ state: "visible" });

  // Crossing the desktop breakpoint cannot leave a stale expanded state.
  await toggle.click();
  assert.equal(await toggle.getAttribute("aria-expanded"), "true");
  await page.setViewportSize({ height: 900, width: 1024 });
  await page.waitForFunction(
    () => document.getElementById("nav-toggle")?.getAttribute("aria-expanded") === "false",
  );
  for (const label of ["Services", "Process", "Frameworks", "About"]) {
    await page.locator(".nav__link", { hasText: label }).first().waitFor({ state: "visible" });
  }

  assert.deepEqual(pageErrors, []);
});
