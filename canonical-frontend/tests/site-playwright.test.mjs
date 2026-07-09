import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

test("playwright renders the canonical.cloud landing page", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
  });
  t.after(() => browser.close());

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
    await page.locator(`.nav__link`, { hasText: label }).first().waitFor({ state: "visible" });
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
