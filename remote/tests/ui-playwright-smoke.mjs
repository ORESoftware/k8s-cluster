import assert from "node:assert/strict";
import { chromium } from "playwright";

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? "http://54.91.17.58").replace(/\/+$/, "");
const targetPath = process.env.REMOTE_DEV_UI_PATH ?? "/agents/tasks";
const targetUrl = `${baseUrl}${targetPath}`;

console.log(`[ui-playwright] target=${targetUrl}`);

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ ignoreHTTPSErrors: true });
const page = await context.newPage();

try {
  const response = await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
  assert.ok(response, `expected ${targetPath} response`);
  assert.equal(response.status(), 200);
  await page.locator("#send-chat").waitFor({ timeout: 30_000 });

  const title = await page.title();
  assert.match(title, /dd agents tasks|dd-remote-web/i);

  const bodyText = (await page.locator("body").innerText()).toLowerCase();
  assert.match(bodyText, /agent tasks/);
  assert.match(bodyText, /thread chat/);
  assert.match(bodyText, /recent tasks/);

  console.log("[ui-playwright] /agents/tasks title/body assertions passed");
  console.log("[ui-playwright] PASS");
} finally {
  await context.close();
  await browser.close();
}
