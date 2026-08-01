// dd-ai-agent-bridge — deployed-URL auth-enforcement browser smoke, run by the
// same remote/tests runner as the DD ui-*-smoke checks. It drives a DEPLOYED
// bridge through BOTH engines (Playwright + Puppeteer) so a regression only one
// driver observes still fails the runner.
//
// This is the browser-automation counterpart to the static
// ai-agent-bridge-k8s-contract.test.mjs: that test proves the manifest WIRES a
// required secret-backed bearer; this proves the SECURITY INVARIANT against live
// traffic — an unauthenticated request to an API route is refused (401/403,
// fails closed) rather than served. The bridge's auth middleware sits in front of
// /agents, /channels, /file-leases, etc.; /healthz and /readyz are intentionally
// outside it so probes never need the token (see remote/deployments/ai-agent-bridge/src/http.rs).
//
// Target: AI_BRIDGE_HTTP_URL (alias AI_BRIDGE_API_TEST_URL), matching the env the
// live ai-agent-bridge-runtime-smoke.mjs already uses. Until a URL is provided the
// smoke SKIPS cleanly (exit 0) — the same tolerance the DD smokes use for an
// absent target — so wiring it into test:all is safe today.
import assert from "node:assert/strict";
import { chromium } from "playwright";
import puppeteer from "puppeteer";

const rawUrl = process.env.AI_BRIDGE_HTTP_URL ?? process.env.AI_BRIDGE_API_TEST_URL ?? "";
const baseUrl = rawUrl.replace(/\/+$/, "");

if (!baseUrl) {
  console.log(
    "[ai-agent-bridge-auth] SKIP: set AI_BRIDGE_HTTP_URL to a deployed bridge to run the smoke.",
  );
  process.exit(0);
}

// An auth-gated route that must never succeed without a credential. GET /agents
// is a read behind the bridge's auth middleware; an unauthenticated caller must
// be denied before any agent inventory is returned.
const probeUrl = `${baseUrl}/agents`;
const healthUrl = `${baseUrl}/healthz`;
// Unauthenticated API access must fail closed. 401/403 are the acceptable
// "denied" codes; 200 is a security failure (served without a bearer) and 5xx is
// a broken endpoint — both must fail the smoke.
const DENIED = new Set([401, 403]);

function assertDenied(engine, status) {
  assert.ok(
    DENIED.has(status),
    `[${engine}] unauthenticated GET ${probeUrl} returned ${status}; expected 401/403 (auth must fail closed)`,
  );
}

async function runPlaywright() {
  console.log(`[ai-agent-bridge-auth/playwright] target=${probeUrl}`);
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  try {
    const health = await page.goto(healthUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(health, "expected a /healthz response");
    assert.equal(health.status(), 200, `expected 200 from ${healthUrl} (is this the bridge?)`);

    const res = await page.goto(probeUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(res, "expected an /agents response");
    assertDenied("playwright", res.status());
    console.log("[ai-agent-bridge-auth/playwright] PASS (denied unauthenticated read)");
  } finally {
    await context.close();
    await browser.close();
  }
}

async function runPuppeteer() {
  console.log(`[ai-agent-bridge-auth/puppeteer] target=${probeUrl}`);
  let browser;
  try {
    browser = await puppeteer.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  } catch (error) {
    // Match the DD puppeteer smoke: fall back to Playwright's Chromium binary
    // when Puppeteer's bundled download is unavailable on the runner.
    console.warn(`[ai-agent-bridge-auth/puppeteer] default launch failed (${error}); using Playwright chromium`);
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
    assert.equal(health.status(), 200, `expected 200 from ${healthUrl} (is this the bridge?)`);

    const res = await page.goto(probeUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    assert.ok(res, "expected an /agents response");
    assertDenied("puppeteer", res.status());
    console.log("[ai-agent-bridge-auth/puppeteer] PASS (denied unauthenticated read)");
  } finally {
    await browser.close();
  }
}

await runPlaywright();
await runPuppeteer();
console.log("[ai-agent-bridge-auth] PASS (both engines: unauthenticated /agents read denied)");
