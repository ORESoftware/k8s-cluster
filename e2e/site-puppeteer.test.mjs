import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { chromeExecutablePath, startServer } from "./harness.mjs";

const pageText = (page) => page.evaluate(() => document.body.innerText);

test("puppeteer renders the akrion-web-server home page", async (t) => {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await puppeteer.launch({
    executablePath: chromeExecutablePath(),
    headless: "new",
    // CI runs Chrome as root, where launch fails without --no-sandbox.
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  await page.setViewport({ height: 900, width: 1440 });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto(`${server.url}/`, { waitUntil: "domcontentloaded" });
  assert.equal(await page.title(), "Akrion Sim");

  // Hero <h1>.
  const h1 = await page.$eval("h1", (el) => el.textContent?.trim());
  assert.equal(h1, "Akrion Sim");

  // Main nav labels.
  const navLinks = await page.$$eval("nav a", (nodes) =>
    nodes.map((n) => n.textContent?.trim()),
  );
  assert.ok(navLinks.includes("Home"), `nav missing Home: ${JSON.stringify(navLinks)}`);
  assert.ok(navLinks.includes("Portal"), `nav missing Portal: ${JSON.stringify(navLinks)}`);

  // Brand text appears in the page.
  assert.match(await pageText(page), /Akrion Sim/);

  assert.deepEqual(pageErrors, []);
});
