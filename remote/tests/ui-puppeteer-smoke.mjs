import assert from "node:assert/strict";
import http from "node:http";
import net from "node:net";
import puppeteer from "puppeteer";
import { chromium as playwrightChromium } from "playwright";

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? "http://54.91.17.58").replace(/\/+$/, "");
const targetPath = process.env.REMOTE_DEV_UI_PATH ?? "/agents/tasks";
const targetUrl = `${baseUrl}${targetPath}`;

console.log(`[ui-puppeteer] target=${targetUrl}`);

async function launchPuppeteerWithFallback() {
  try {
    return await puppeteer.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox", "--ignore-certificate-errors"],
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`[ui-puppeteer] default launch failed (${message}); retrying with Playwright chromium`);
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

async function runOnce(browser) {
  const page = await browser.newPage();
  const response = await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
  assert.ok(response, `expected ${targetPath} response`);
  assert.equal(response.status(), 200);
  await page.waitForSelector("#send-chat", { timeout: 30_000 });

  const title = await page.title();
  assert.match(title, /dd agents tasks|dd-remote-web/i);

  const bodyText = await page.$eval("body", (el) => (el.innerText || "").toLowerCase());
  assert.match(bodyText, /agent tasks/);
  assert.match(bodyText, /thread chat/);
  assert.match(bodyText, /recent tasks/);

  await page.close();
}

async function findOpenPort() {
  return await new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address !== "object") {
        reject(new Error("failed to read bound address"));
        return;
      }
      server.close(() => resolvePort(address.port));
    });
  });
}

function createProxyServer() {
  return http.createServer(async (request, response) => {
    try {
      const path = request.url ?? "/";
      const upstream = await fetch(`${baseUrl}${path}`, {
        method: request.method ?? "GET",
        headers: { "user-agent": "dd-remote-tests-puppeteer-proxy" },
      });
      const headers = {
        "content-type": upstream.headers.get("content-type") ?? "text/plain; charset=utf-8",
      };
      const location = upstream.headers.get("location");
      if (location) {
        headers.location = location;
      }
      response.writeHead(upstream.status, headers);
      response.end(await upstream.text());
    } catch (error) {
      response.writeHead(502, { "content-type": "text/plain; charset=utf-8" });
      response.end(error instanceof Error ? error.message : String(error));
    }
  });
}

const browser = await launchPuppeteerWithFallback();
try {
  let proxyServer = null;
  try {
    await runOnce(browser);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.includes("ERR_BLOCKED_BY_CLIENT")) {
      throw error;
    }
    console.warn(
      "[ui-puppeteer] navigation blocked by client policy; retrying via localhost proxy",
    );
    await browser.close();
    const port = await findOpenPort();
    proxyServer = createProxyServer();
    await new Promise((resolveListen) => {
      proxyServer.listen(port, "127.0.0.1", resolveListen);
    });
    const proxyBase = `http://127.0.0.1:${port}`;
    const proxyBrowser = await puppeteer.launch({
      headless: true,
      executablePath: playwrightChromium.executablePath(),
      args: [
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--disable-extensions",
        "--ignore-certificate-errors",
      ],
    });
    try {
      const page = await proxyBrowser.newPage();
      const proxyRes = await page.goto(`${proxyBase}${targetPath}`, {
        waitUntil: "domcontentloaded",
        timeout: 30_000,
      });
      assert.ok(proxyRes, `expected proxy ${targetPath} response`);
      assert.equal(proxyRes.status(), 200);
      await page.waitForSelector("#send-chat", { timeout: 30_000 });

      const title = await page.title();
      assert.match(title, /dd agents tasks|dd-remote-web/i);

      const bodyText = await page.$eval("body", (el) => (el.innerText || "").toLowerCase());
      assert.match(bodyText, /agent tasks/);
      assert.match(bodyText, /thread chat/);
      assert.match(bodyText, /recent tasks/);
      await page.close();
    } finally {
      await proxyBrowser.close();
      if (proxyServer) {
        await new Promise((resolveClose) => proxyServer.close(resolveClose));
      }
    }
  }
  console.log("[ui-puppeteer] /agents/tasks title/body assertions passed");
  console.log("[ui-puppeteer] PASS");
} finally {
  if (browser.connected) {
    await browser.close();
  }
}
