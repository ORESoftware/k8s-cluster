// Browser smoke for the benefactor.cc marketing site, run by the cluster's
// Playwright/Puppeteer runners. It exercises the same public site the repo's own
// GitHub Actions gate (benefactor-cc/benefactor-cc.github.io) covers, but against
// the *live* deployment so the runner catches a broken publish.
//
// Both engines run in one process against BENEFACTOR_SITE_URL (default the live
// site). Each asserts the home hero/nav and the client-side unsubscribe mailto
// builder — the one piece of interactive behavior on the site.
import assert from "node:assert/strict";
import puppeteer from "puppeteer";
import { chromium as playwrightChromium } from "playwright";

const baseUrl = (process.env.BENEFACTOR_SITE_URL ?? "https://benefactor.cc").replace(/\/+$/, "");
const HERO = "Leaner funnels. Sharper intent. Compounding growth.";

console.log(`[benefactor-smoke] target=${baseUrl}`);

// Shared assertions, engine-agnostic: given helpers that return the page title,
// the hero <h1> text, the lowercased body text, and the unsubscribe mailto href.
async function assertHome({ title, hero, body }) {
  assert.match(title, /Benefactor/i, "home <title> lost the brand");
  assert.equal(hero.replace(/\s+/g, " ").trim(), HERO, "hero headline changed");
  for (const label of ["services", "process", "results", "contact"]) {
    assert.match(body, new RegExp(label, "i"), `home nav is missing "${label}"`);
  }
}

function assertUnsubscribe(href, { campaign, leadId }) {
  assert.ok(
    href && href.startsWith("mailto:hello@benefactor.cc?subject=Unsubscribe&body="),
    `unexpected unsubscribe href: ${href}`,
  );
  const decoded = decodeURIComponent(href.split("body=")[1]);
  assert.match(decoded, new RegExp(`Campaign: ${campaign}`), "unsubscribe campaign not wired");
  assert.match(decoded, new RegExp(`Lead: ${leadId}`), "unsubscribe leadId not wired");
}

// ---- Puppeteer -------------------------------------------------------------
async function runPuppeteer() {
  let browser;
  try {
    browser = await puppeteer.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`[benefactor-smoke] puppeteer default launch failed (${message}); using playwright chromium`);
    browser = await puppeteer.launch({
      headless: true,
      executablePath: playwrightChromium.executablePath(),
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  }
  try {
    const home = await browser.newPage();
    const res = await home.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(res, "expected a home response");
    assert.equal(res.status(), 200, "home did not return 200");
    await assertHome({
      title: await home.title(),
      hero: await home.$eval("h1", (el) => el.textContent ?? ""),
      body: (await home.$eval("body", (el) => el.innerText || "")).toLowerCase(),
    });
    await home.close();

    const unsub = await browser.newPage();
    await unsub.goto(`${baseUrl}/unsubscribe/?campaign=cluster-smoke&leadId=probe-1`, {
      waitUntil: "networkidle0",
      timeout: 60_000,
    });
    assertUnsubscribe(
      await unsub.$eval("#unsubscribe-link", (el) => el.getAttribute("href")),
      { campaign: "cluster-smoke", leadId: "probe-1" },
    );
    await unsub.close();
  } finally {
    if (browser.connected) await browser.close();
  }
  console.log("[benefactor-smoke] puppeteer PASS");
}

// ---- Playwright ------------------------------------------------------------
async function runPlaywright() {
  const browser = await playwrightChromium.launch({
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  try {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const home = await context.newPage();
    const res = await home.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(res, "expected a home response");
    assert.equal(res.status(), 200, "home did not return 200");
    await assertHome({
      title: await home.title(),
      hero: await home.locator("h1").first().innerText(),
      body: (await home.locator("body").innerText()).toLowerCase(),
    });

    const unsub = await context.newPage();
    await unsub.goto(`${baseUrl}/unsubscribe/?campaign=cluster-smoke&leadId=probe-2`, {
      waitUntil: "networkidle",
      timeout: 60_000,
    });
    assertUnsubscribe(await unsub.locator("#unsubscribe-link").getAttribute("href"), {
      campaign: "cluster-smoke",
      leadId: "probe-2",
    });
    await context.close();
  } finally {
    await browser.close();
  }
  console.log("[benefactor-smoke] playwright PASS");
}

await runPuppeteer();
await runPlaywright();
console.log("[benefactor-smoke] PASS");
