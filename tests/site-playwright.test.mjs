// Playwright browser E2E: boots `astro preview` and asserts the rendered landing
// page and custom 404 (title, hero, nav, service cards, CTAs) load error-free.
import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { assertVisibleText } from "@fiducia/test-config/assert";
import {
  chromeExecutablePath,
  sitePath,
  startSite,
} from "./site-browser-harness.mjs";

test("playwright renders the Fiducia marketing landing page and custom 404", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    args: process.env.CI === "true" ? ["--no-sandbox", "--disable-dev-shm-usage"] : [],
    executablePath: chromeExecutablePath(),
    headless: true,
  });
  t.after(() => browser.close());

  const page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  // --- Landing page at the configured deployment base -----------------------
  await page.goto(`${server.url}${sitePath()}`, { waitUntil: "networkidle" });
  assert.equal(await page.title(), "Fiducia — Consensus & Coordination as a Service");

  // Hero <h1> — the headline is split across a <br>, so match the normalized
  // text rather than exact-equal.
  const hero = page.getByRole("heading", { level: 1 });
  await hero.waitFor({ state: "visible" });
  assert.match(
    (await hero.innerText()).replace(/\s+/g, " ").trim(),
    /Consensus & Coordination\s*as a Service/,
  );

  // Nav: logo text, the four section links, and the primary CTA.
  await assertVisibleText(page, "Fiducia.cloud");
  for (const label of ["Services", "AI agents", "How it works", "Why Raft"]) {
    await page.getByRole("link", { name: label, exact: true }).waitFor({ state: "visible" });
  }
  // Two a.btn--primary[href="#start"] exist (nav "Get started" + hero
  // "Start building"); scope to the nav CTA by its text.
  const getStarted = page.locator('a.btn--primary[href="#start"]').filter({ hasText: "Get started" });
  await getStarted.waitFor({ state: "visible" });
  assert.equal(await getStarted.count(), 1);

  // The six coordination-primitive service cards.
  for (const service of [
    "Locks & Semaphores",
    "Rate Limiting",
    "Cron & Scheduling",
    "Config KV & Watches",
    "Leader Election",
    "Service Discovery",
  ]) {
    await page.getByRole("heading", { name: service, exact: true }).waitFor({ state: "visible" });
  }

  // CTA link to the API uses the same configured deployment base.
  const apiLink = page.getByRole("link", { name: "View the API", exact: true });
  await apiLink.waitFor({ state: "visible" });
  assert.equal(await apiLink.getAttribute("href"), sitePath("api/info"));

  // Footer.
  await assertVisibleText(page, "© 2026 Fiducia");

  // --- Custom 404 ------------------------------------------------------------
  // A genuinely-missing path yields astro-preview's own bare 404, so load the
  // built custom 404 directly (served 200) to assert its content.
  await page.goto(`${server.url}${sitePath("404.html")}`, { waitUntil: "networkidle" });
  assert.equal(await page.title(), "Not found — Fiducia");
  await assertVisibleText(page, "quorum on this page");

  assert.deepEqual(pageErrors, []);
});
