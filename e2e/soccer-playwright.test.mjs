import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startServer } from "./harness.mjs";

test("playwright renders the /soccer live UI", async (t) => {
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

  await page.goto(`${server.url}/soccer`, { waitUntil: "domcontentloaded" });
  assert.equal(await page.title(), "Live Soccer Simulation");

  // Hero heading + the live scoreboard scaffold.
  await page.getByRole("heading", { level: 1, name: "Live Soccer Simulation" }).first().waitFor({ state: "visible" });
  await page.locator(".app").first().waitFor({ state: "attached" });
  await page.locator(".score").first().waitFor({ state: "attached" });

  // The bare liveness probe answers 200.
  const health = await page.request.get(`${server.url}/healthz`);
  assert.ok(health.ok(), `/healthz not ok: ${health.status()}`);
});
