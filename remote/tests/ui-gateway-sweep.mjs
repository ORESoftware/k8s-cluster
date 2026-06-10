import fs from "node:fs/promises";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const execFileAsync = promisify(execFile);
const defaultBaseUrl = "https://54.91.17.58";
const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? process.env.DD_GATEWAY_BASE_URL ?? defaultBaseUrl).replace(
  /\/+$/,
  "",
);
const serverAuthSecret = process.env.REMOTE_DEV_SERVER_SECRET ?? process.env.SERVER_AUTH_SECRET ?? "";
const gatewayAuth =
  process.env.ALL_DOGS ?? process.env.REMOTE_DEV_AUTH_COOKIE ?? process.env.DD_AUTH_COOKIE_VALUE ?? "";
const outputRoot = path.resolve(
  repoRoot,
  process.env.REMOTE_DEV_UI_REPORT_DIR ?? "tmp/ui-gateway-sweep",
);
const runId =
  process.env.REMOTE_DEV_UI_REPORT_RUN_ID ??
  new Date().toISOString().replace(/[:.]/g, "-").replace(/Z$/, "Z");
const outputDir = path.join(outputRoot, runId);
const settleMs = Number.parseInt(process.env.REMOTE_DEV_UI_SETTLE_MS ?? "500", 10);
const connectTimeoutMs = Number.parseInt(process.env.REMOTE_DEV_UI_CONNECT_TIMEOUT_MS ?? "10000", 10);
const sourceRef =
  process.argv.find((arg) => arg.startsWith("--source-ref="))?.slice("--source-ref=".length) ??
  process.env.REMOTE_DEV_UI_SOURCE_REF ??
  "";

const publicUi = "public-ui";
const protectedUi = "protected-ui";
const publicStatus = "public-status";
const protectedStatus = "protected-status";

const targetOverrides = new Map(
  [
    { name: "root redirect", path: "/", kind: publicUi, expect: /remote|service directory/i },
    { name: "service directory", path: "/home", kind: publicUi, expect: /service directory|managed runtime/i },
    { name: "central API docs", path: "/api-docs", kind: publicUi, expect: /generated api documentation|service/i },
    { name: "central API docs JSON", path: "/api-docs.json", kind: publicStatus, expect: /generated-api-docs|services/i },
    { name: "web-home API docs", path: "/docs/api", kind: publicUi, expect: /web-home-rs api docs|route/i },
    { name: "web-home API docs alias", path: "/api/docs", kind: publicUi, expect: /web-home-rs api docs|route/i },
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
    { name: "fsharp websocket", path: "/fsws/", kind: protectedUi, expect: /f#|websocket|rx|async/i },
    { name: "dev server agents", path: "/agents", kind: protectedUi, expect: /agents|providers|remote/i },
    { name: "dev server status", path: "/status", kind: protectedStatus, expect: /status|ok|health/i },
    { name: "container pools", path: "/container-pools", kind: protectedUi, expect: /container|pool|warm/i },
    { name: "build server", path: "/builds", kind: protectedStatus, expect: /^\s*\[|build|jobs|logs/i },
    {
      name: "bastion inventory",
      path: "/bastion/runtime/deployments",
      kind: protectedStatus,
      expect: /deployment|pod|container/i,
    },
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
  ].map((target) => [target.path, target]),
);

const manualTargets = [...targetOverrides.values()];

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

function isStatusTarget(pathname) {
  return /(?:\/(?:healthz|readyz|livez|metrics|status|schema|example|api-docs(?:\.json)?|docs\/api|api\/docs(?:\.json)?)|\.json)(?:[/?#]|$)/i.test(
    pathname,
  );
}

function isAuthPage(url, text) {
  const authFormUrl = (() => {
    try {
      const pathname = new URL(url).pathname.replace(/\/+$/, "");
      return pathname === "/auth";
    } catch {
      return /\/auth(?:[?#]|\/(?:[?#]|$)|$)/.test(url);
    }
  })();
  return authFormUrl || /missing required dd header|passphrase|dd remote auth/i.test(text);
}

function summarizeText(text) {
  return text.replace(/\s+/g, " ").trim().slice(0, 220);
}

function shouldSkipDerivedTarget(label, href) {
  if (!href || !href.startsWith("/")) {
    return true;
  }
  if (/^POST\s+/i.test(label)) {
    return true;
  }
  if (href === "/auth/logout") {
    return true;
  }
  return [
    /^\/api\/agents\/threads\/example-thread-id\//,
    /^\/builds\/example-job(?:\/|$)/,
    /^\/dd-thread\/example(?:\/|$)/,
    /^\/lambdas\/invoke\/00000000-0000-0000-0000-000000000000$/,
    /^\/stream\/example-task-id$/,
  ].some((pattern) => pattern.test(href));
}

function kindFor(access, href) {
  const protectedAccess = access !== "PUBLIC";
  const status = isStatusTarget(href);
  if (protectedAccess) {
    return status ? protectedStatus : protectedUi;
  }
  return status ? publicStatus : publicUi;
}

async function sourceDerivedTargets() {
  const sourceRepoPath = "remote/deployments/web-home-rs/src/main.rs";
  const sourcePath = path.join(repoRoot, sourceRepoPath);
  const source = sourceRef
    ? (await execFileAsync("git", ["show", `${sourceRef}:${sourceRepoPath}`], { cwd: repoRoot, maxBuffer: 5 * 1024 * 1024 }))
        .stdout
    : await fs.readFile(sourcePath, "utf8");
  const rowPattern = /PathRow\s*\{\s*paths:\s*&\[(?<paths>[\s\S]*?)\],\s*target:\s*"(?<target>[^"]+)",\s*access:\s*(?<access>[A-Z_]+)/g;
  const entryPattern = /PathEntry\s*\{\s*label:\s*(?:"(?<label>[^"]+)"|[A-Z_]+),\s*href:\s*Some\("(?<href>[^"]+)"\)\s*\}/g;
  const targets = [];

  for (const row of source.matchAll(rowPattern)) {
    const target = row.groups?.target ?? "service-directory";
    const access = row.groups?.access ?? "SERVER_AUTH";
    const paths = row.groups?.paths ?? "";

    for (const entry of paths.matchAll(entryPattern)) {
      const label = entry.groups?.label ?? "";
      const href = entry.groups?.href ?? "";
      if (shouldSkipDerivedTarget(label, href)) {
        continue;
      }
      targets.push({
        name: `${target}: ${label || href}`,
        path: href,
        kind: kindFor(access, href),
        source: "web-home-service-directory",
      });
    }
  }

  return targets;
}

async function buildTargets() {
  const merged = new Map();
  for (const target of await sourceDerivedTargets()) {
    merged.set(target.path, target);
  }
  for (const target of manualTargets) {
    merged.set(target.path, { ...(merged.get(target.path) ?? {}), ...target, source: "manual-or-override" });
  }
  return [...merged.values()].sort((a, b) => a.path.localeCompare(b.path));
}

async function main() {
  await fs.mkdir(outputDir, { recursive: true });
  const targets = await buildTargets();

  if (process.argv.includes("--list")) {
    for (const target of targets) {
      console.log(`${target.kind.padEnd(16)} ${target.path} ${target.name}`);
    }
    console.log(`[ui-sweep] total=${targets.length}`);
    return;
  }

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

  if (!process.argv.includes("--skip-preflight")) {
    const preflightPage = await context.newPage();
    try {
      const response = await preflightPage.goto(`${baseUrl}/`, {
        waitUntil: "domcontentloaded",
        timeout: Number.isFinite(connectTimeoutMs) ? connectTimeoutMs : 10_000,
      });
      const status = response?.status() ?? 0;
      if (status === 404 || status >= 500) {
        const reportPath = path.join(outputDir, "report.json");
        const result = {
          name: "gateway preflight",
          path: "/",
          kind: publicStatus,
          outcome: "fail",
          status,
          finalUrl: preflightPage.url(),
          title: "",
          reason: `preflight failed before route sweep: HTTP ${status}`,
          consoleErrors: [],
          pageErrors: [],
          failedRequests: [],
          notFoundResponses: status === 404 ? [`${status} ${baseUrl}/`] : [],
          gatewayResponses: [502, 503, 504].includes(status) ? [`${status} ${baseUrl}/`] : [],
          serverResponses: status >= 500 && ![502, 503, 504].includes(status) ? [`${status} ${baseUrl}/`] : [],
          screenshot: "",
        };
        await fs.writeFile(
          reportPath,
          `${JSON.stringify({ baseUrl, hasGatewayAuth, hasServerAuth, preflightOnly: true, results: [result] }, null, 2)}\n`,
        );
        console.log(`[ui-sweep] FAIL  /                                  ${status} ${result.reason}`);
        console.log(`[ui-sweep] report=${path.relative(process.cwd(), reportPath)}`);
        process.exitCode = 1;
        await preflightPage.close().catch(() => {});
        await context.close();
        await browser.close();
        return;
      }
    } catch (error) {
      const reportPath = path.join(outputDir, "report.json");
      const reason = error instanceof Error ? error.message : String(error);
      const result = {
        name: "gateway preflight",
        path: "/",
        kind: publicStatus,
        outcome: "fail",
        status: 0,
        finalUrl: `${baseUrl}/`,
        title: "",
        reason: `preflight failed before route sweep: ${reason}`,
        consoleErrors: [],
        pageErrors: [],
        failedRequests: [],
        notFoundResponses: [],
        gatewayResponses: [],
        serverResponses: [],
        screenshot: "",
      };
      await fs.writeFile(
        reportPath,
        `${JSON.stringify({ baseUrl, hasGatewayAuth, hasServerAuth, preflightOnly: true, results: [result] }, null, 2)}\n`,
      );
      console.log(`[ui-sweep] FAIL  /                                  0 ${result.reason}`);
      console.log(`[ui-sweep] report=${path.relative(process.cwd(), reportPath)}`);
      process.exitCode = 1;
      await preflightPage.close().catch(() => {});
      await context.close();
      await browser.close();
      return;
    } finally {
      await preflightPage.close().catch(() => {});
    }
  }

  for (const target of targets) {
    const page = await context.newPage();
    const consoleErrors = [];
    const pageErrors = [];
    const failedRequests = [];
    const notFoundResponses = [];
    const gatewayResponses = [];
    const serverResponses = [];

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
      if (!url.startsWith(baseUrl)) {
        return;
      }
      if (response.status() === 404) {
        notFoundResponses.push(`${response.status()} ${url}`);
      } else if ([502, 503, 504].includes(response.status())) {
        gatewayResponses.push(`${response.status()} ${url}`);
      } else if (response.status() >= 500) {
        serverResponses.push(`${response.status()} ${url}`);
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
      await page.waitForTimeout(Number.isFinite(settleMs) ? settleMs : 500);
      title = await page.title().catch(() => "");
      text = await page.locator("body").innerText({ timeout: 5_000 }).catch(() => "");

      const authGated = isAuthPage(finalUrl, text);
      const matchedText = target.expect ? target.expect.test(`${title}\n${text}`) : true;
      const missing = status === 404 || notFoundResponses.length > 0;
      const gatewayError = [502, 503, 504].includes(status) || gatewayResponses.length > 0;
      const serverError = status >= 500 || serverResponses.length > 0;
      const expectedAuthGate =
        isProtected(target.kind) && !hasGatewayAuth && (authGated || status === 401 || status === 403);
      const unexpectedAuthGate =
        authGated && !target.allowAuthPage && (!isProtected(target.kind) || hasGatewayAuth);

      if (status === 0) {
        reason = "navigation failed before an HTTP response was received";
      } else if (gatewayError) {
        reason = `gateway/upstream error: main=${status}, subresources=${gatewayResponses.join("; ")}`;
      } else if (serverError) {
        reason = `server error: main=${status}, subresources=${serverResponses.join("; ")}`;
      } else if (missing) {
        reason = `not found: main=${status}, subresources=${notFoundResponses.join("; ")}`;
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
      notFoundResponses,
      gatewayResponses,
      serverResponses,
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
