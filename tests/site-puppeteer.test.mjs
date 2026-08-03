// Puppeteer browser E2E: boots `astro preview` and asserts the rendered landing
// page and custom 404 (title, hero, nav, service cards, CTAs) load error-free.
import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { pageText } from "@fiducia/test-config/assert";
import {
  chromeExecutablePath,
  sitePath,
  startSite,
} from "./site-browser-harness.mjs";

test("puppeteer renders the Fiducia marketing landing page and custom 404", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await puppeteer.launch({
    args: process.env.CI === "true"
      ? ["--no-sandbox", "--disable-setuid-sandbox", "--disable-dev-shm-usage"]
      : [],
    executablePath: chromeExecutablePath(),
    headless: true,
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  await page.setViewport({ height: 900, width: 1440 });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  // --- Landing page at the configured deployment base -----------------------
  await page.goto(`${server.url}${sitePath()}`, { waitUntil: "networkidle0" });
  assert.equal(await page.title(), "Fiducia — Consensus & Coordination as a Service");

  // Hero <h1> — the headline is split across a <br>, so normalize whitespace
  // before matching.
  const heroTitle = await page.$eval("h1.hero__title", (element) =>
    (element.textContent ?? "").replace(/\s+/g, " ").trim(),
  );
  assert.match(heroTitle, /Consensus & Coordination\s*as a Service/);

  // Nav: logo text and the four section link labels, in order.
  assert.equal(
    await page.$eval(".nav__logo-text", (element) => element.textContent?.trim()),
    "Fiducia.cloud",
  );
  const navLinks = await page.$$eval(".nav__link", (nodes) =>
    nodes.map((node) => node.textContent?.trim()),
  );
  assert.deepEqual(navLinks, ["Services", "AI agents", "How it works", "Why Raft"]);

  // Two a.btn--primary[href="#start"] exist (nav "Get started" + hero
  // "Start building"); assert the nav CTA is among them.
  const primaryStartCtas = await page.$$eval('a.btn--primary[href="#start"]', (nodes) =>
    nodes.map((node) => node.textContent?.trim()),
  );
  assert.ok(
    primaryStartCtas.includes("Get started"),
    `expected a "Get started" primary CTA, saw: ${JSON.stringify(primaryStartCtas)}`,
  );

  // The six coordination-primitive service cards under #services, in order.
  const serviceCards = await page.$$eval("#services .card h3", (nodes) =>
    nodes.map((node) => node.textContent?.trim()),
  );
  assert.deepEqual(serviceCards, [
    "Locks & Semaphores",
    "Rate Limiting",
    "Cron & Scheduling",
    "Config KV & Watches",
    "Leader Election",
    "Service Discovery",
  ]);

  // CTA link to the API uses the same configured deployment base.
  const apiHref = sitePath("api/info");
  assert.equal(
    await page.$eval(`a[href="${apiHref}"]`, (element) => element.textContent?.trim()),
    "View the API",
  );

  // Footer.
  assert.match(await pageText(page), /© 2026 Fiducia/);

  // --- Custom 404 ------------------------------------------------------------
  // A genuinely-missing path yields astro-preview's own bare 404, so load the
  // built custom 404 directly (served 200) to assert its content.
  await page.goto(`${server.url}${sitePath("404.html")}`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.title === "Not found — Fiducia");
  assert.match(await pageText(page), /quorum on this page/);

  assert.deepEqual(pageErrors, []);
});
