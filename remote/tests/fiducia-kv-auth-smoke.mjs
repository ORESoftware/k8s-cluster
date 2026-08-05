// fiducia.cloud KV data-plane — deployed-URL auth-enforcement browser smoke,
// run by the same remote/tests runner as the DD ui-*-smoke checks. It drives a
// DEPLOYED api.fiducia.cloud through BOTH engines (Playwright + Puppeteer) so a
// regression only one driver observes still fails the runner.
//
// This is the browser-automation counterpart to the fiducia-secret-delivery
// static contract test: it proves the SECURITY INVARIANT against live traffic —
// the public KV endpoint refuses an unauthenticated read (scope-gated, fails
// closed) rather than leaking secret material. See docs/fiducia-secret-delivery.md
// and the FID-SEC-1 hardening (DEN-1240).
//
// Target: FIDUCIA_API_TEST_URL (alias FIDUCIA_API_BASE_URL). Until the data
// plane is deployed the URL is unset and the smoke SKIPS cleanly (exit 0) — the
// same tolerance the DD smokes use for an absent target — so wiring it into
// test:all is safe today.
import assert from "node:assert/strict";
import { chromium } from "playwright";
import puppeteer from "puppeteer";

const rawUrl = process.env.FIDUCIA_API_TEST_URL ?? process.env.FIDUCIA_API_BASE_URL ?? "";
const baseUrl = rawUrl.replace(/\/+$/, "");

if (!baseUrl) {
  console.log(
    "[fiducia-kv-auth] SKIP: set FIDUCIA_API_TEST_URL to a deployed api.fiducia.cloud to run the smoke.",
  );
  process.exit(0);
}

// A read that must never succeed without a credential. The key is nonexistent
// and namespaced under a smoke prefix so a misconfigured target cannot return a
// real value even if auth were (wrongly) disabled.
const probeKey = "k8s/smoke/fiducia-kv-auth/NONEXISTENT";
const kvUrl = `${baseUrl}/v1/kv?key=${encodeURIComponent(probeKey)}`;
const healthUrl = `${baseUrl}/healthz`;
// Unauthenticated KV access must fail closed. 401/403 are the acceptable
// "denied" codes; 200 is a security failure (leaked/served without a key) and
// 5xx is a broken endpoint — both must fail the smoke.
const DENIED = new Set([401, 403]);

function assertDenied(engine, status) {
  assert.ok(
    DENIED.has(status),
    `[${engine}] unauthenticated GET ${kvUrl} returned ${status}; expected 401/403 (auth must fail closed)`,
  );
}

async function runPlaywright() {
  console.log(`[fiducia-kv-auth/playwright] target=${kvUrl}`);
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  try {
    const health = await page.goto(healthUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(health, "expected a /healthz response");
    assert.equal(health.status(), 200, `expected 200 from ${healthUrl} (is this the LB?)`);

    const res = await page.goto(kvUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(res, "expected a KV response");
    assertDenied("playwright", res.status());
    console.log("[fiducia-kv-auth/playwright] PASS (denied unauthenticated read)");
  } finally {
    await context.close();
    await browser.close();
  }
}

async function runPuppeteer() {
  console.log(`[fiducia-kv-auth/puppeteer] target=${kvUrl}`);
  let browser;
  try {
    browser = await puppeteer.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  } catch (error) {
    // Match the DD puppeteer smoke: fall back to Playwright's Chromium binary
    // when Puppeteer's bundled download is unavailable on the runner.
    console.warn(`[fiducia-kv-auth/puppeteer] default launch failed (${error}); using Playwright chromium`);
    browser = await puppeteer.launch({
      headless: true,
      executablePath: chromium.executablePath(),
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  }
  try {
    const page = await browser.newPage();
    const health = await page.goto(healthUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(health, "expected a /healthz response");
    assert.equal(health.status(), 200, `expected 200 from ${healthUrl} (is this the LB?)`);

    const res = await page.goto(kvUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(res, "expected a KV response");
    assertDenied("puppeteer", res.status());
    console.log("[fiducia-kv-auth/puppeteer] PASS (denied unauthenticated read)");
  } finally {
    await browser.close();
  }
}

await runPlaywright();
await runPuppeteer();
console.log("[fiducia-kv-auth] PASS (both engines: unauthenticated KV read denied)");
