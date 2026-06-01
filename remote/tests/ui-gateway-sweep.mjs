import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const defaultBaseUrl = "https://54.91.17.58";
const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? defaultBaseUrl).replace(/\/+$/, "");
const serverAuthSecret = process.env.REMOTE_DEV_SERVER_SECRET ?? process.env.SERVER_AUTH_SECRET ?? "";
const gatewayAuth = process.env.REMOTE_DEV_AUTH_COOKIE ?? process.env.DD_AUTH_COOKIE_VALUE ?? "";
const outputDir = path.resolve(
  repoRoot,
  process.env.REMOTE_DEV_UI_REPORT_DIR ?? "tmp/ui-gateway-sweep",
);

const publicUi = "public-ui";
const protectedUi = "protected-ui";
const publicStatus = "public-status";
const protectedStatus = "protected-status";

const targets = [
  { name: "root redirect", path: "/", kind: publicUi, expect: /remote|service directory/i },
  { name: "service directory", path: "/home", kind: publicUi, expect: /service directory|managed runtime/i },
  { name: "agent tasks", path: "/agents/tasks", kind: protectedUi, expect: /agent tasks|thread chat|recent tasks/i },
  { name: "agent threads", path: "/agents/threads", kind: protectedUi, expect: /agent threads|thread control|response stream/i },
  { name: "lambda functions", path: "/lambdas/functions", kind: protectedUi, expect: /lambda functions|function body/i },
  {
    name: "presence lab",
    path: "/presence-test?user=alice&device=d1",
    kind: publicUi,
    expect: /presence|conversation|device/i,
  },
  { name: "websocket lab", path: "/wss-test", kind: publicUi, expect: /websocket|preset|connect/i },
  {
    name: "websocket lab gleam",
    path: "/wss-test?preset=gleam",
    kind: publicUi,
    expect: /websocket|gleam|connect/i,
  },
  {
    name: "websocket lab webrtc",
    path: "/wss-test?preset=webrtc",
    kind: publicUi,
    expect: /websocket|webrtc|connect/i,
  },
  {
    name: "websocket lab gcs",
    path: "/wss-test?preset=gcs",
    kind: publicUi,
    expect: /websocket|gcs|connect/i,
  },
  {
    name: "websocket lab fsrx",
    path: "/wss-test?preset=fsrx",
    kind: publicUi,
    expect: /websocket|fsrx|connect|rx/i,
  },
  {
    name: "auth form",
    path: "/auth?return=/home",
    kind: publicUi,
    expect: /passphrase|auth|sign in/i,
    allowAuthPage: true,
  },
  { name: "webrtc signaling", path: "/webrtc/", kind: protectedUi, expect: /webrtc|signal/i },
  { name: "mdp optimizer", path: "/mdp/", kind: protectedUi, expect: /mdp|optimizer|healthz/i },
  { name: "des simulator", path: "/des/", kind: protectedUi, expect: /des|simulation|model/i },
  { name: "des music production", path: "/des/music", kind: protectedUi, expect: /music production|music-sample-seed|breakbeat/i },
  { name: "fsharp websocket", path: "/fsws/", kind: protectedUi, expect: /f#|websocket|rx|async/i },
  { name: "dev server agents", path: "/agents", kind: protectedUi, expect: /agents|providers|remote/i },
  { name: "dev server status", path: "/status", kind: protectedStatus, expect: /status|ok|health/i },
  { name: "container pools", path: "/container-pools", kind: protectedUi, expect: /container|pool|warm/i },
  // /builds is a JSON listing endpoint (GET returns the build job
  // array). It does not serve HTML, so classify it as a protected
  // status surface and accept either the JSON envelope or a populated
  // job list.
  { name: "build server", path: "/builds", kind: protectedStatus, expect: /^\s*\[|build|jobs|logs/i },
  { name: "bastion inventory", path: "/bastion/runtime/deployments", kind: protectedStatus, expect: /deployment|pod|container/i },
  { name: "headlamp", path: "/headlamp/", kind: protectedUi, expect: /headlamp|kubernetes|token/i },
  { name: "gleam service", path: "/gleam/home", kind: protectedUi, expect: /gleam|websocket|connect/i },
  { name: "mcp metadata", path: "/mcp", kind: protectedStatus, expect: /mcp|json-rpc|tools/i },
  { name: "mcp home", path: "/mcp/home", kind: protectedUi, expect: /mcp|observability|json-rpc/i },
  { name: "contract service", path: "/contracts/", kind: protectedUi, expect: /contract|solana|schema/i },
  { name: "ml pipeline", path: "/ml/", kind: protectedUi, expect: /ml|pipeline|telemetry|analyze/i },
  { name: "trading service", path: "/trading/", kind: protectedUi, expect: /trading|decision|schema/i },
  { name: "web scraper strategies", path: "/scrape/strategies", kind: protectedStatus, expect: /playwright|puppeteer|strategy/i },
  { name: "grafana", path: "/telemetry/", kind: protectedUi, expect: /grafana|telemetry|dashboard/i },
  { name: "prometheus", path: "/prometheus/", kind: protectedUi, expect: /prometheus|query/i },
  { name: "nats monitor", path: "/nats/", kind: protectedUi, expect: /nats|varz|connection/i },
  { name: "reaper status", path: "/reaper/", kind: protectedStatus, expect: /reaper|idle|sweep|cron/i },
  { name: "cron status", path: "/cron/", kind: protectedStatus, expect: /cron|scheduler|reaper/i },
  { name: "gcs health", path: "/gcs/health", kind: protectedStatus, expect: /ok|health|status/i },
];

function slugify(value) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 80);
}

function isProtected(kind) {
  return kind.startsWith("protected");
}

function isAuthPage(url, text) {
  return /\/auth(?:[/?#]|$)/.test(url) || /missing required dd header|passphrase|dd remote auth/i.test(text);
}

function summarizeText(text) {
  return text.replace(/\s+/g, " ").trim().slice(0, 220);
}

async function main() {
  await fs.rm(outputDir, { recursive: true, force: true });
  await fs.mkdir(outputDir, { recursive: true });

  const extraHTTPHeaders = {};
  if (serverAuthSecret) {
    extraHTTPHeaders["X-Server-Auth"] = serverAuthSecret;
  }
  if (gatewayAuth) {
    extraHTTPHeaders.Auth = gatewayAuth;
  }

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    baseURL: baseUrl,
    ignoreHTTPSErrors: true,
    viewport: { width: 1440, height: 1000 },
    extraHTTPHeaders,
  });

  if (gatewayAuth) {
    const parsed = new URL(baseUrl);
    await context.addCookies([
      {
        name: "dd_auth",
        value: gatewayAuth,
        domain: parsed.hostname,
        path: "/",
        httpOnly: true,
        secure: parsed.protocol === "https:",
        sameSite: "Lax",
      },
    ]);
  }

  const hasGatewayAuth = Boolean(gatewayAuth);
  const hasServerAuth = Boolean(serverAuthSecret);
  const results = [];

  for (const target of targets) {
    const page = await context.newPage();
    const consoleErrors = [];
    const pageErrors = [];
    const failedRequests = [];
    const badResponses = [];

    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("requestfailed", (request) => {
      const failure = request.failure();
      failedRequests.push(`${request.method()} ${request.url()} ${failure?.errorText ?? ""}`.trim());
    });
    page.on("response", (response) => {
      const url = response.url();
      if (url.startsWith(baseUrl) && response.status() >= 500) {
        badResponses.push(`${response.status()} ${url}`);
      }
    });

    const url = `${baseUrl}${target.path}`;
    let status = 0;
    let finalUrl = url;
    let title = "";
    let text = "";
    let outcome = "fail";
    let reason = "";
    let screenshot = "";

    try {
      const response = await page.goto(url, { waitUntil: "domcontentloaded", timeout: 45_000 });
      status = response?.status() ?? 0;
      finalUrl = page.url();
      await page.waitForTimeout(1_000);
      title = await page.title().catch(() => "");
      text = await page.locator("body").innerText({ timeout: 5_000 }).catch(() => "");

      const authGated = isAuthPage(finalUrl, text);
      const matchedText = target.expect.test(`${title}\n${text}`);
      const serverError = status >= 500 || badResponses.length > 0;
      const expectedAuthGate =
        isProtected(target.kind) && !hasGatewayAuth && (authGated || status === 401 || status === 403);
      const unexpectedAuthGate =
        authGated && !target.allowAuthPage && (!isProtected(target.kind) || hasGatewayAuth);

      if (serverError) {
        reason = `server error: main=${status}, subresources=${badResponses.join("; ")}`;
      } else if (unexpectedAuthGate) {
        reason = hasGatewayAuth
          ? "auth credentials were supplied, but navigation still reached auth/unauthorized page"
          : "public target unexpectedly reached auth/unauthorized page";
      } else if (expectedAuthGate) {
        outcome = "auth-gated";
        reason = "protected target redirected/answered with gateway auth as expected";
      } else if (status >= 400) {
        reason = `unexpected HTTP ${status}`;
      } else if (!matchedText) {
        reason = `expected text did not match; saw: ${summarizeText(text)}`;
      } else if (pageErrors.length > 0) {
        reason = `page errors: ${pageErrors.join("; ")}`;
      } else if (consoleErrors.length > 0) {
        outcome = "warn";
        reason = `console errors: ${consoleErrors.slice(0, 3).join("; ")}`;
      } else if (failedRequests.length > 0) {
        outcome = "warn";
        reason = `request failures: ${failedRequests.slice(0, 3).join("; ")}`;
      } else {
        outcome = "pass";
        reason = "loaded and matched expected content";
      }

      if (outcome !== "pass" && outcome !== "auth-gated") {
        screenshot = path.join(outputDir, `${slugify(target.name)}.png`);
        await page.screenshot({ path: screenshot, fullPage: true }).catch(() => {});
      }
    } catch (error) {
      reason = error instanceof Error ? error.message : String(error);
      screenshot = path.join(outputDir, `${slugify(target.name)}.png`);
      await page.screenshot({ path: screenshot, fullPage: true }).catch(() => {});
    } finally {
      await page.close();
    }

    const result = {
      name: target.name,
      path: target.path,
      kind: target.kind,
      outcome,
      status,
      finalUrl,
      title,
      reason,
      consoleErrors,
      pageErrors,
      failedRequests,
      badResponses,
      screenshot: screenshot ? path.relative(repoRoot, screenshot) : "",
    };
    results.push(result);
    const mark = outcome === "pass" ? "PASS" : outcome === "auth-gated" ? "AUTH" : outcome.toUpperCase();
    console.log(`[ui-sweep] ${mark.padEnd(5)} ${target.path.padEnd(34)} ${status} ${reason}`);
  }

  await context.close();
  await browser.close();

  const reportPath = path.join(outputDir, "report.json");
  await fs.writeFile(
    reportPath,
    `${JSON.stringify({ baseUrl, hasGatewayAuth, hasServerAuth, results }, null, 2)}\n`,
  );

  const failed = results.filter((result) => result.outcome === "fail");
  const warned = results.filter((result) => result.outcome === "warn");
  const gated = results.filter((result) => result.outcome === "auth-gated");
  const passed = results.filter((result) => result.outcome === "pass");

  console.log(
    `[ui-sweep] summary pass=${passed.length} auth-gated=${gated.length} warn=${warned.length} fail=${failed.length}`,
  );
  console.log(`[ui-sweep] report=${path.relative(process.cwd(), reportPath)}`);

  if (failed.length > 0) {
    process.exitCode = 1;
  }
}

await main();
