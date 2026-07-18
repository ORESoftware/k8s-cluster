import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

// Second Puppeteer spec file (alongside site-puppeteer.test.mjs): the landing
// page is long, so these split the below-the-fold sections into focused checks
// that fail with a precise message when a section regresses.

async function open(t) {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await puppeteer.launch({
    executablePath: chromeExecutablePath(),
    headless: "new",
    // --no-sandbox: CI runners drive Chrome as root, where launch otherwise
    // fails ("Running as root without --no-sandbox"); harmless locally.
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  await page.setViewport({ height: 900, width: 1440 });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.goto(`${server.url}/`, { waitUntil: "networkidle0" });
  return { page, pageErrors };
}

test("puppeteer: stats bar shows the four proof-point metrics in order", async (t) => {
  const { page, pageErrors } = await open(t);

  const stats = await page.$$eval("#stats-bar .stats__item", (items) =>
    items.map((item) => ({
      number: item.querySelector(".stats__number")?.textContent?.trim(),
      label: item.querySelector(".stats__label")?.textContent?.trim(),
    })),
  );
  assert.deepEqual(stats, [
    { number: "60%", label: "Lower Cost vs Big 4" },
    { number: "4–8", label: "Weeks to Audit-Ready" },
    { number: "100%", label: "First-Pass Success Rate" },
    { number: "24/7", label: "Continuous Monitoring" },
  ]);
  assert.deepEqual(pageErrors, []);
});

test("puppeteer: process timeline lists the four ordered engagement steps", async (t) => {
  const { page, pageErrors } = await open(t);

  const steps = await page.$$eval("#process .process__step", (nodes) =>
    nodes.map((node) => ({
      number: node.querySelector(".process__step-number")?.textContent?.trim(),
      title: node.querySelector("h3")?.textContent?.trim(),
    })),
  );
  assert.deepEqual(steps, [
    { number: "01", title: "Scoping & Gap Analysis" },
    { number: "02", title: "Policy & Control Design" },
    { number: "03", title: "Audit Execution" },
    { number: "04", title: "Report & Continuous Monitoring" },
  ]);
  assert.deepEqual(pageErrors, []);
});

test("puppeteer: hero CTAs target the right sections and advertise the framework badges", async (t) => {
  const { page, pageErrors } = await open(t);

  assert.equal(
    await page.$eval("#hero-cta-primary", (el) => el.getAttribute("href")),
    "#contact",
  );
  assert.equal(
    await page.$eval("#hero-cta-secondary", (el) => el.getAttribute("href")),
    "#services",
  );

  const badges = await page.$$eval(".hero__framework-badge", (nodes) =>
    nodes.map((n) => n.textContent?.replace(/\s+/g, " ").trim()),
  );
  assert.deepEqual(badges, ["SOC 2", "FedRAMP", "HIPAA", "ISO 27001"]);
  assert.deepEqual(pageErrors, []);
});

test("puppeteer: clicking a nav link performs same-page anchor navigation", async (t) => {
  const { page, pageErrors } = await open(t);

  // "Services" is the first nav link; clicking it should move the URL hash to
  // #services without a full navigation, and the target section must exist.
  const servicesLink = await page.$$eval(".nav__link", (nodes) =>
    nodes.findIndex((n) => n.textContent?.trim() === "Services"),
  );
  assert.ok(servicesLink >= 0, "expected a Services nav link");

  const links = await page.$$(".nav__link");
  await links[servicesLink].click();
  await page.waitForFunction(() => location.hash === "#services");

  assert.equal(new URL(page.url()).hash, "#services");
  assert.equal(
    await page.$eval("#services", (el) => el.tagName.toLowerCase()),
    "section",
  );
  assert.deepEqual(pageErrors, []);
});
