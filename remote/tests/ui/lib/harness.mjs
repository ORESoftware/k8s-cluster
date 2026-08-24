// Shared browser harness for the node:test athleto UI suites.
//
// The same feature assertions run under BOTH engines (Playwright's bundled
// chromium and Puppeteer) so a regression that only one engine surfaces still
// fails CI. Production targets are fixed by the workflow. Local preview hosts
// must be public HTTPS origins explicitly listed in ATHLETO_UI_ALLOWED_ORIGINS.

import { chromium } from "playwright";
import puppeteer from "puppeteer";

import { normalizeLiveTarget } from "./live-targets.mjs";

export const MARKETING_URL = normalizeLiveTarget(
  "ATHLETO_MARKETING_URL",
  process.env.ATHLETO_MARKETING_URL ?? "https://athleto.store",
);
export const APP_URL = normalizeLiveTarget(
  "ATHLETO_APP_URL",
  process.env.ATHLETO_APP_URL ?? "https://app.athleto.store",
);

// Live sites sit behind Cloudflare / an in-pod-built pod; give navigation room.
export const NAV_TIMEOUT = Number(process.env.ATHLETO_UI_NAV_TIMEOUT_MS ?? 60_000);

if (!Number.isFinite(NAV_TIMEOUT) || NAV_TIMEOUT < 1_000 || NAV_TIMEOUT > 120_000) {
  throw new TypeError('ATHLETO_UI_NAV_TIMEOUT_MS must be between 1000 and 120000');
}

const LAUNCH_ARGS = [
  "--no-sandbox",
  "--disable-setuid-sandbox",
  "--disable-dev-shm-usage",
];

/** Launch Playwright's bundled chromium. */
export async function launchPlaywright() {
  const browser = await chromium.launch({ headless: true, args: LAUNCH_ARGS });
  return {
    engine: "playwright",
    browser,
    async newPage() {
      const context = await browser.newContext();
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
