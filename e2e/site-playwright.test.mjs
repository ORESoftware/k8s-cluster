import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startServer } from "./harness.mjs";

test("playwright renders the akrion-web-server home page", async (t) => {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    // CI runs Chrome as root, where the sandbox refuses to start.
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto(`${server.url}/`, { waitUntil: "domcontentloaded" });
  assert.equal(await page.title(), "Akrion Sim");

  // Hero heading.
  const hero = page.getByRole("heading", { level: 1, name: "Akrion Sim" }).first();
  await hero.waitFor({ state: "visible" });

  // Main nav: Home + Portal.
  for (const label of ["Home", "Portal"]) {
    await page.getByRole("link", { name: label, exact: true }).first().waitFor({ state: "visible" });
  }

  // Theme switcher is present (dark/medium/light radios).
  await page.locator('[data-theme-option="dark"]').first().waitFor({ state: "visible" });

  assert.deepEqual(pageErrors, []);
});
