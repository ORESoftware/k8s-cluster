// fiducia.cloud customer portal — deployed-URL browser smoke, run by the same
// remote/tests runner harness as the DD ui-*-smoke checks. It drives a DEPLOYED
// portal through BOTH engines (Playwright + Puppeteer) so a regression only one
// driver observes still fails the runner.
//
// Target: FIDUCIA_CUSTOMER_TEST_URL (alias FIDUCIA_CUSTOMER_BASE_URL). The
// portal shell is static, so the assertions hold against any deployed build.
// Until a portal is deployed the URL is unset and the smoke SKIPS cleanly
// (exit 0) rather than failing — identical to how the DD smokes tolerate an
// absent target — so wiring it into test:all is safe today.
import assert from "node:assert/strict";
import { chromium } from "playwright";
import puppeteer from "puppeteer";

const rawUrl = process.env.FIDUCIA_CUSTOMER_TEST_URL ?? process.env.FIDUCIA_CUSTOMER_BASE_URL ?? "";
const baseUrl = rawUrl.replace(/\/+$/, "");
const targetPath = process.env.FIDUCIA_CUSTOMER_UI_PATH ?? "/app";

if (!baseUrl) {
  console.log(
    "[fiducia-customer] SKIP: set FIDUCIA_CUSTOMER_TEST_URL to a deployed portal to run the smoke.",
  );
  process.exit(0);
}

const targetUrl = `${baseUrl}${targetPath}`;
// Landmarks present in the static portal shell (index.html), engine-agnostic.
const REQUIRED_TEXT = [/Customer workspace/, /API keys/, /Security/, /Settings/];

function assertShell(engine, bodyText) {
  for (const pattern of REQUIRED_TEXT) {
    assert.match(bodyText, pattern, `[${engine}] portal shell missing ${pattern}`);
  }
}

async function runPlaywright() {
  console.log(`[fiducia-customer/playwright] target=${targetUrl}`);
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  try {
    const response = await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(response, "expected a response");
    assert.equal(response.status(), 200, `expected 200 from ${targetUrl}`);
    await page.getByText("Customer workspace").first().waitFor({ state: "visible", timeout: 30_000 });
    assertShell("playwright", await page.locator("body").innerText());
    assert.deepEqual(pageErrors, [], "[playwright] uncaught page errors");
    console.log("[fiducia-customer/playwright] PASS");
  } finally {
    await context.close();
    await browser.close();
  }
}

async function runPuppeteer() {
  console.log(`[fiducia-customer/puppeteer] target=${targetUrl}`);
  let browser;
  try {
    browser = await puppeteer.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  } catch (error) {
    // Match the DD puppeteer smoke: fall back to Playwright's Chromium binary
    // when Puppeteer's bundled download is unavailable on the runner.
    console.warn(`[fiducia-customer/puppeteer] default launch failed (${error}); using Playwright chromium`);
    browser = await puppeteer.launch({
      headless: true,
      executablePath: chromium.executablePath(),
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  }
  try {
    const page = await browser.newPage();
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    const response = await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(response, "expected a response");
    assert.equal(response.status(), 200, `expected 200 from ${targetUrl}`);
    await page.waitForFunction(() => document.body.textContent?.includes("Customer workspace"), {
      timeout: 30_000,
    });
    assertShell("puppeteer", await page.$eval("body", (body) => body.innerText));
    assert.deepEqual(pageErrors, [], "[puppeteer] uncaught page errors");
    console.log("[fiducia-customer/puppeteer] PASS");
  } finally {
    await browser.close();
  }
}

await runPlaywright();
await runPuppeteer();
console.log("[fiducia-customer] PASS (both engines)");
