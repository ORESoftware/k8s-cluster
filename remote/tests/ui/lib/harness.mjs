// Shared browser harness for the node:test athleto UI suites.
//
// The same feature assertions run under BOTH engines (Playwright's bundled
// chromium and Puppeteer) so a regression that only one engine surfaces still
// fails CI. Targets are live public URLs and are env-overridable:
//   ATHLETO_MARKETING_URL  default https://athleto.store        (GitHub Pages via Cloudflare)
//   ATHLETO_APP_URL        default https://app.athleto.store    (storefront pod behind the ingress)
// Set either to a preview/staging origin to point the whole suite elsewhere.

import { chromium } from "playwright";
import puppeteer from "puppeteer";

const stripTrailingSlash = (value) => value.replace(/\/+$/, "");

export const MARKETING_URL = stripTrailingSlash(
  process.env.ATHLETO_MARKETING_URL ?? "https://athleto.store",
);
export const APP_URL = stripTrailingSlash(
  process.env.ATHLETO_APP_URL ?? "https://app.athleto.store",
);

// Live sites sit behind Cloudflare / an in-pod-built pod; give navigation room.
export const NAV_TIMEOUT = Number(process.env.ATHLETO_UI_NAV_TIMEOUT_MS ?? 60_000);

const LAUNCH_ARGS = [
  "--no-sandbox",
  "--disable-setuid-sandbox",
  "--disable-dev-shm-usage",
  "--ignore-certificate-errors",
];

/** Launch Playwright's bundled chromium. */
export async function launchPlaywright() {
  const browser = await chromium.launch({ headless: true, args: LAUNCH_ARGS });
  return {
    engine: "playwright",
    browser,
    async newPage() {
      const context = await browser.newContext({ ignoreHTTPSErrors: true });
      const page = await context.newPage();
      page.setDefaultTimeout(NAV_TIMEOUT);
      return { page, context };
    },
    close: () => browser.close(),
  };
}

/**
 * Launch Puppeteer, falling back to Playwright's chromium binary when
 * Puppeteer's own download is absent (the pattern the existing UI smokes use,
 * so the suite runs on hosts that only provisioned one browser).
 */
export async function launchPuppeteer() {
  let browser;
  try {
    browser = await puppeteer.launch({ headless: true, args: LAUNCH_ARGS });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`[athleto-ui] puppeteer default launch failed (${message}); using playwright chromium`);
    browser = await puppeteer.launch({
      headless: true,
      executablePath: chromium.executablePath(),
      args: LAUNCH_ARGS,
    });
  }
  return {
    engine: "puppeteer",
    browser,
    async newPage() {
      const page = await browser.newPage();
      page.setDefaultNavigationTimeout(NAV_TIMEOUT);
      page.setDefaultTimeout(NAV_TIMEOUT);
      return { page, context: undefined };
    },
    close: () => browser.close(),
  };
}

/** Navigate and return the response, retrying once for a cold/rebuilding pod. */
export async function gotoWithRetry(page, url, { waitUntil = "domcontentloaded" } = {}) {
  let lastError;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const response = await page.goto(url, { waitUntil, timeout: NAV_TIMEOUT });
      return response;
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

/** Engine-agnostic status code from a Playwright or Puppeteer response. */
export const statusOf = (response) => (typeof response?.status === "function" ? response.status() : undefined);
