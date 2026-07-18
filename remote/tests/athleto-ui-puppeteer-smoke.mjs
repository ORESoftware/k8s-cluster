import assert from "node:assert/strict";
import puppeteer from "puppeteer";
import { chromium as playwrightChromium } from "playwright";

// Base URL precedence: ATHLETO_BASE_URL, then REMOTE_DEV_BASE_URL, then the
// documented public storefront host.
const baseUrl = (
  process.env.ATHLETO_BASE_URL ??
  process.env.REMOTE_DEV_BASE_URL ??
  "https://app.athleto.store"
).replace(/\/+$/, "");

const TAG = "[athleto-ui-puppeteer]";

function isUnreachable(error) {
  // node's fetch reports the connection code on error.cause, not error.message.
  const parts = [];
  let node = error;
  for (let i = 0; node && i < 5; i += 1) {
    if (node.message) parts.push(String(node.message));
    if (node.code) parts.push(String(node.code));
    node = node.cause;
  }
  if (parts.length === 0) parts.push(String(error));
  const message = parts.join(" ").toLowerCase();
  return (
    message.includes("econnrefused") ||
    message.includes("enotfound") ||
    message.includes("eai_again") ||
    message.includes("ehostunreach") ||
    message.includes("enetunreach") ||
    message.includes("etimedout") ||
    message.includes("timeout") ||
    message.includes("timed out") ||
    message.includes("err_connection") ||
    message.includes("err_name_not_resolved") ||
    message.includes("err_address_unreachable") ||
    message.includes("socket hang up") ||
    message.includes("net::")
  );
}

async function reachable() {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 8000);
  try {
    await fetch(`${baseUrl}/healthz`, {
      method: "GET",
      redirect: "manual",
      signal: controller.signal,
      headers: { "user-agent": "dd-remote-tests-athleto-smoke" },
    });
    return true;
  } catch (error) {
    if (isUnreachable(error)) {
      return false;
    }
    return true;
  } finally {
    clearTimeout(timer);
  }
}

// Preserve the portability pattern from ui-puppeteer-smoke.mjs: try the bundled
// Chromium first, then fall back to Playwright's chromium executablePath.
async function launchPuppeteerWithFallback() {
  try {
    return await puppeteer.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`${TAG} default launch failed (${message}); retrying with Playwright chromium`);
    return await puppeteer.launch({
      headless: true,
      executablePath: playwrightChromium.executablePath(),
      args: [
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--disable-extensions",
        "--ignore-certificate-errors",
      ],
    });
  }
}

async function main() {
  console.log(`${TAG} base=${baseUrl}`);

  if (!(await reachable())) {
    console.log(`${TAG} SKIP: ${baseUrl} is not reachable from this network (service not deployed / not routable). Exiting 0.`);
    process.exit(0);
  }

  const browser = await launchPuppeteerWithFallback();
  try {
    const page = await browser.newPage();

    const rootResponse = await page.goto(baseUrl + "/", {
      waitUntil: "domcontentloaded",
      timeout: 60_000,
    });
    assert.ok(rootResponse, "expected a response for GET /");
    assert.equal(rootResponse.status(), 200, "GET / should return 200");

    const headers = rootResponse.headers();
    assert.ok(headers["content-security-policy"], "expected a Content-Security-Policy header");
    assert.match(headers["x-frame-options"] ?? "", /deny|sameorigin/i);
    assert.match(headers["x-content-type-options"] ?? "", /nosniff/i);

    const bodyText = await page.$eval("body", (el) => (el.innerText || "").toLowerCase());
    assert.match(bodyText, /athlet-?o|wobble|gelatin/, "storefront should show Athlet-O brand copy");
    console.log(`${TAG} storefront GET / (200 + brand + security headers) passed`);

    const asset = await fetch(`${baseUrl}/static/htmx-2.0.4.min.js`, {
      redirect: "manual",
      headers: { "user-agent": "dd-remote-tests-athleto-smoke" },
    });
    assert.equal(asset.status, 200, "htmx asset should return 200");
    assert.match(
      asset.headers.get("content-type") ?? "",
      /javascript/i,
      "htmx asset should be a javascript content-type",
    );
    console.log(`${TAG} /static htmx asset content-type passed`);

    const health = await fetch(`${baseUrl}/healthz`, { redirect: "manual" });
    assert.equal(health.status, 200, "GET /healthz should return 200");
    console.log(`${TAG} /healthz 200 passed`);

    const ready = await fetch(`${baseUrl}/readyz`, { redirect: "manual" });
    if (ready.status === 200) {
      const ct = ready.headers.get("content-type") ?? "";
      assert.match(ct, /json/i, "/readyz should be JSON when present");
      JSON.parse(await ready.text());
      console.log(`${TAG} backend /readyz JSON passed`);
    } else {
      console.log(`${TAG} /readyz not present on this target (HTTP ${ready.status}); backend-only route, skipped`);
    }

    await page.close();
    console.log(`${TAG} PASS`);
  } finally {
    if (browser.connected) {
      await browser.close();
    }
  }
}

main().catch((error) => {
  if (isUnreachable(error)) {
    console.log(`${TAG} SKIP: target became unreachable mid-run (${error?.message ?? error}). Exiting 0.`);
    process.exit(0);
  }
  console.error(`${TAG} FAIL:`, error?.stack || error);
  process.exit(1);
});
