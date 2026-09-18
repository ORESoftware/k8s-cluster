import assert from "node:assert/strict";
import { test } from "node:test";
import puppeteer from "puppeteer";
import { chromeExecutablePath, startServer } from "./harness.mjs";

test("puppeteer renders the /soccer live UI", async (t) => {
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

  await page.goto(`${server.url}/soccer`, { waitUntil: "domcontentloaded" });
  assert.equal(await page.title(), "Live Soccer Simulation");

  // Hero <h1> and the live scoreboard scaffold.
  const h1 = await page.$eval("h1", (el) => el.textContent?.trim());
  assert.equal(h1, "Live Soccer Simulation");
  assert.equal(await page.$eval(".score", (el) => Boolean(el)), true);

  // The bare liveness probe answers 200.
  const health = await fetch(`${server.url}/healthz`);
  assert.ok(health.ok, `/healthz not ok: ${health.status}`);
});
