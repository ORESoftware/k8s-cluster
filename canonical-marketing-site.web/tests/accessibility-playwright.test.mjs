import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

async function launchBrowser(t) {
  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());
  return browser;
}

async function assertNoHorizontalOverflow(page, label) {
  const dimensions = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  assert.ok(
    dimensions.scrollWidth <= dimensions.clientWidth + 1,
    `${label} overflowed horizontally: ${JSON.stringify(dimensions)}`,
  );
}

test("static markup keeps landmarks, fragments, and mobile state truthful without JavaScript", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());
  const browser = await launchBrowser(t);
  const context = await browser.newContext({
    javaScriptEnabled: false,
    viewport: { height: 844, width: 390 },
  });
  t.after(() => context.close());
  const page = await context.newPage();

  await page.goto(`${server.url}/`, { waitUntil: "domcontentloaded" });

  const skipLink = page.getByRole("link", { name: "Skip to main content", exact: true });
  assert.equal(await skipLink.getAttribute("href"), "#main-content");

  assert.equal(await page.getByRole("navigation", { name: "Primary navigation" }).count(), 1);
  assert.equal(await page.getByRole("navigation", { name: "Footer navigation" }).count(), 1);

  const main = page.getByRole("main");
  assert.equal(await main.getAttribute("id"), "main-content");
  assert.equal(await main.getAttribute("tabindex"), "-1");

  const toggle = page.locator("#nav-toggle");
  assert.equal(await toggle.getAttribute("type"), "button");
  assert.equal(await toggle.getAttribute("aria-controls"), "nav-links");
  assert.equal(await toggle.getAttribute("aria-expanded"), "false");
  assert.equal(await toggle.getAttribute("aria-label"), "Open navigation");
  assert.equal(await page.locator("#nav-links").isVisible(), false);

  const integrity = await page.evaluate(() => {
    const ids = [...document.querySelectorAll("[id]")].map((element) => element.id);
    const duplicates = [...new Set(ids.filter((id, index) => ids.indexOf(id) !== index))];
    const missingFragments = [...document.querySelectorAll('a[href^="#"]')]
      .map((anchor) => anchor.getAttribute("href"))
      .filter((href) => href && href.length > 1)
      .filter((href) => document.getElementById(decodeURIComponent(href.slice(1))) === null);
    return { duplicates, missingFragments };
  });
  assert.deepEqual(integrity, { duplicates: [], missingFragments: [] });

  const decorativeLogos = page.locator("svg.nav__logo-icon");
  assert.ok((await decorativeLogos.count()) >= 2);
  for (let index = 0; index < (await decorativeLogos.count()); index += 1) {
    const logo = decorativeLogos.nth(index);
    assert.equal(await logo.getAttribute("aria-hidden"), "true");
    assert.equal(await logo.getAttribute("focusable"), "false");
  }

  await assertNoHorizontalOverflow(page, "JavaScript-disabled mobile page");
});

test("the first keyboard action exposes skip navigation and transfers focus", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());
  const browser = await launchBrowser(t);
  const page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto(`${server.url}/`, { waitUntil: "networkidle" });
  await page.keyboard.press("Tab");

  const skipLink = page.getByRole("link", { name: "Skip to main content", exact: true });
  assert.equal(await page.evaluate(() => document.activeElement?.classList.contains("skip-link")), true);
  await page.waitForFunction(() => {
    const link = document.querySelector(".skip-link");
    return link instanceof HTMLElement && link.getBoundingClientRect().top >= 0;
  });
  const focusStyle = await skipLink.evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      outlineStyle: style.outlineStyle,
      outlineWidth: Number.parseFloat(style.outlineWidth),
      top: element.getBoundingClientRect().top,
    };
  });
  assert.equal(focusStyle.outlineStyle, "solid");
  assert.ok(focusStyle.outlineWidth >= 3, JSON.stringify(focusStyle));
  assert.ok(focusStyle.top >= 0, JSON.stringify(focusStyle));

  await page.keyboard.press("Enter");
  await page.waitForURL(/#main-content$/);
  await page.waitForFunction(() => document.activeElement?.id === "main-content");
  assert.equal(await page.evaluate(() => document.activeElement?.id), "main-content");
  assert.deepEqual(pageErrors, []);
});

test("reduced motion and responsive widths remain browser-enforced", async (t) => {
  const server = await startSite();
  t.after(() => server.stop());
  const browser = await launchBrowser(t);
  const page = await browser.newPage({ viewport: { height: 900, width: 1440 } });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto(`${server.url}/`, { waitUntil: "networkidle" });

  const motion = await page.evaluate(() => {
    const toMilliseconds = (value) =>
      value
        .split(",")
        .map((part) => part.trim())
        .filter(Boolean)
        .map((part) => (part.endsWith("ms") ? Number.parseFloat(part) : Number.parseFloat(part) * 1000));
    const animated = getComputedStyle(document.querySelector(".animate-fade-in-up"));
    const transitioned = getComputedStyle(document.querySelector(".btn"));
    return {
      animationMs: toMilliseconds(animated.animationDuration),
      transitionMs: toMilliseconds(transitioned.transitionDuration),
      scrollBehavior: getComputedStyle(document.documentElement).scrollBehavior,
    };
  });
  assert.ok(motion.animationMs.every((duration) => duration <= 0.01), JSON.stringify(motion));
  assert.ok(motion.transitionMs.every((duration) => duration <= 0.01), JSON.stringify(motion));
  assert.equal(motion.scrollBehavior, "auto");

  await assertNoHorizontalOverflow(page, "desktop page");
  await page.setViewportSize({ height: 844, width: 390 });
  await page.waitForFunction(() => window.innerWidth === 390);
  await assertNoHorizontalOverflow(page, "mobile page");
});
