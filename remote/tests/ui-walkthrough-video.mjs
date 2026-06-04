import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

// Captures a short demo video of the authenticated UI sweep against the
// live EC2 gateway. Pages are visited in order, with brief settle pauses
// so the recording shows each surface clearly.

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? "https://54.91.17.58").replace(/\/+$/, "");
const authCookie = process.env.REMOTE_DEV_AUTH_COOKIE ?? process.env.DD_AUTH_COOKIE_VALUE ?? "";
const outputDir = path.resolve(
  repoRoot,
  process.env.REMOTE_DEV_VIDEO_DIR ?? "tmp/ui-walkthrough-video",
);

const tour = [
  { path: "/home", label: "Service directory" },
  { path: "/agents/tasks", label: "Agent tasks (public)" },
  { path: "/agents/threads", label: "Agent threads (public)" },
  { path: "/lambdas/functions", label: "Lambda functions (auth)" },
  { path: "/wss-test?preset=gleam", label: "WebSocket lab — Gleam" },
  { path: "/wss-test?preset=webrtc", label: "WebSocket lab — WebRTC" },
  { path: "/container-pools", label: "Container pools (auth)" },
  { path: "/builds", label: "Build server (JSON, auth)" },
  { path: "/bastion/runtime/deployments", label: "Bastion inventory (auth)" },
  { path: "/headlamp/", label: "Headlamp (auth)" },
  { path: "/gleam/home", label: "Gleam WS service (auth)" },
  { path: "/mcp/home", label: "MCP server (auth)" },
  { path: "/contracts/", label: "Solana contracts (auth)" },
  { path: "/ml/", label: "AI/ML pipeline (auth)" },
  { path: "/trading/", label: "Trading server (auth)" },
  { path: "/scrape/strategies", label: "Web scraper (auth)" },
  { path: "/telemetry/", label: "Grafana telemetry (auth)" },
  { path: "/prometheus/", label: "Prometheus (auth)" },
  { path: "/nats/", label: "NATS monitor (auth)" },
  { path: "/reaper/", label: "Idle reaper (auth)" },
  { path: "/cron/", label: "Cron scheduler (auth)" },
];

async function main() {
  await fs.rm(outputDir, { recursive: true, force: true });
  await fs.mkdir(outputDir, { recursive: true });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    baseURL: baseUrl,
    ignoreHTTPSErrors: true,
    viewport: { width: 1440, height: 900 },
    recordVideo: { dir: outputDir, size: { width: 1440, height: 900 } },
  });

  if (authCookie) {
    const parsed = new URL(baseUrl);
    await context.addCookies([
      {
        name: "dd_auth",
        value: authCookie,
        domain: parsed.hostname,
        path: "/",
        httpOnly: true,
        secure: parsed.protocol === "https:",
        sameSite: "Lax",
      },
    ]);
  }

  const page = await context.newPage();
  for (const stop of tour) {
    const target = `${baseUrl}${stop.path}`;
    console.log(`[walkthrough] ${stop.label} -> ${stop.path}`);
    try {
      await page.goto(target, { waitUntil: "domcontentloaded", timeout: 45_000 });
    } catch (error) {
      console.warn(`[walkthrough] failed ${stop.path}: ${error instanceof Error ? error.message : String(error)}`);
      continue;
    }
    await page.waitForTimeout(1_500);
  }

  await page.close();
  await context.close();
  await browser.close();

  const videoFiles = (await fs.readdir(outputDir)).filter((name) => name.endsWith(".webm"));
  console.log(`[walkthrough] saved ${videoFiles.length} clip(s) under ${path.relative(process.cwd(), outputDir)}`);
}

await main();
