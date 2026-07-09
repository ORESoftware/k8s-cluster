import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

const pageText = (page) => page.evaluate(() => document.body.innerText);

test("puppeteer renders the canonical.cloud landing page", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await puppeteer.launch({
    executablePath: chromeExecutablePath(),
    headless: "new",
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  await page.setViewport({ height: 900, width: 1440 });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto(`${server.url}/`, { waitUntil: "networkidle0" });
  assert.equal(await page.title(), "SOC 2, FedRAMP & HIPAA Compliance Audits | canonical.cloud");

  // Hero <h1> — headline split across a <br>, so normalize whitespace.
  const heroTitle = await page.$eval(".hero__title", (el) =>
    (el.textContent ?? "").replace(/\s+/g, " ").trim(),
  );
  assert.match(heroTitle, /Compliance Audits\s*Without the Overhead/);

  // Nav brand.
  const brand = await page.$eval(".nav__logo-text", (el) =>
    (el.textContent ?? "").replace(/\s+/g, "").trim(),
  );
  assert.match(brand, /CANONICAL\.CLOUD/);

  // Nav section links, in order.
  const navLinks = await page.$$eval(".nav__link", (nodes) =>
    nodes.map((n) => n.textContent?.trim()),
  );
  assert.deepEqual(navLinks, ["Services", "Process", "Frameworks", "About"]);

  // The four compliance service cards under #services, in order.
  const serviceCards = await page.$$eval("#services .services__card h3", (nodes) =>
    nodes.map((n) => n.textContent?.trim()),
  );
  assert.deepEqual(serviceCards, [
    "SOC 2 Attestation",
    "FedRAMP Authorization",
    "HIPAA Compliance",
    "vCISO & IT Advisory",
  ]);

  // Primary contact CTA (mailto).
  assert.equal(
    await page.$eval('a[href="mailto:compliance@canonical.cloud"]', (el) => Boolean(el)),
    true,
  );

  // Footer copyright.
  assert.match(await pageText(page), /canonical\.cloud\. All rights reserved/);

  assert.deepEqual(pageErrors, []);
});
