import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { writeBrowserFailureDiagnostics } from "./browser-failure-diagnostics.mjs";
import { chromeExecutablePath, startSite } from "./site-browser-harness.mjs";

const requireContainerHeaders = process.env.CANONICAL_REQUIRE_SECURITY_HEADERS === "1";
const artifactDirectory = process.env.CANONICAL_BROWSER_ARTIFACT_DIR?.trim();

test(
  "playwright verifies the shipped web surface enforces browser security policy",
  { skip: !requireContainerHeaders },
  async (t) => {
    const server = await startSite();
    t.after(() => server.stop());
    const expectedOrigin = new URL(server.url).origin;
    const targetUrl = `${server.url}/`;

    const browser = await chromium.launch({
      executablePath: chromeExecutablePath(),
      headless: true,
      args: ["--no-sandbox", "--disable-setuid-sandbox"],
    });
    t.after(() => browser.close());

    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      serviceWorkers: "block",
    });
    t.after(() => context.close());
    await context.tracing.start({ screenshots: true, snapshots: true, sources: false });

    const page = await context.newPage();
    const externalRequests = [];
    const failedRequests = [];
    const pageErrors = [];
    const consoleMessages = [];
    let response;
    let tracingStopped = false;

    page.on("request", (request) => {
      if (new URL(request.url()).origin !== expectedOrigin) {
        externalRequests.push(request.url());
      }
    });
    page.on("requestfailed", (request) => {
      failedRequests.push({
        method: request.method(),
        url: request.url(),
        errorText: request.failure()?.errorText ?? "unknown",
      });
    });
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("console", (message) => {
      consoleMessages.push({ type: message.type(), text: message.text() });
    });

    try {
      response = await page.goto(targetUrl, { waitUntil: "networkidle" });
      assert.ok(response);
      assert.equal(response.status(), 200);

      const finalUrl = new URL(response.url());
      assert.equal(
        finalUrl.origin,
        expectedOrigin,
        "the deployed origin must not redirect browser trust to another host",
      );

      // Use the complete wire-visible header set. Playwright's compact headers()
      // view can omit security-sensitive fields on some response paths.
      const headers = await response.allHeaders();
      assert.match(headers["content-type"], /^text\/html\b/i);
      assert.equal(headers["x-content-type-options"], "nosniff");
      assert.equal(headers["x-frame-options"], "DENY");
      assert.equal(headers["referrer-policy"], "strict-origin-when-cross-origin");
      assert.equal(headers["cross-origin-opener-policy"], "same-origin");
      assert.match(headers["permissions-policy"], /camera=\(\)/);
      assert.match(headers["permissions-policy"], /microphone=\(\)/);

      if (finalUrl.protocol === "https:") {
        assert.match(
          headers["strict-transport-security"],
          /^max-age=\d+(?:;|$)/,
          "the live HTTPS origin must advertise HSTS",
        );
      }

      const csp = headers["content-security-policy"];
      assert.ok(csp, "response must include a Content-Security-Policy");
      for (const directive of [
        "default-src 'self'",
        "script-src 'self'",
        "base-uri 'self'",
        "form-action 'self'",
        "frame-ancestors 'none'",
        "object-src 'none'",
      ]) {
        assert.match(csp, new RegExp(directive.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      }
      assert.doesNotMatch(csp, /script-src[^;]*'unsafe-inline'/);
      assert.doesNotMatch(csp, /script-src[^;]*\*/);

      // Exercise the policy in Chromium rather than merely parsing the header.
      await page.evaluate(() => {
        const script = document.createElement("script");
        script.textContent = "window.__canonicalInlineScriptExecuted = true";
        document.head.append(script);
      });
      await page.waitForTimeout(100);
      assert.equal(
        await page.evaluate(() => window.__canonicalInlineScriptExecuted),
        undefined,
        "the response CSP must block executable inline script",
      );

      // The landing page should not silently expand its network/CSP trust surface.
      assert.deepEqual(externalRequests, []);
      assert.deepEqual(pageErrors, []);
    } catch (error) {
      try {
        const result = await writeBrowserFailureDiagnostics({
          artifactDirectory,
          context,
          page,
          response,
          targetUrl,
          error,
          externalRequests,
          failedRequests,
          pageErrors,
          consoleMessages,
        });
        tracingStopped = result.tracingStopped;
      } catch (diagnosticError) {
        process.stderr.write(
          `browser diagnostic capture failed: ${diagnosticError?.message ?? diagnosticError}\n`,
        );
      }
      throw error;
    } finally {
      if (!tracingStopped) {
        await context.tracing.stop().catch((error) => {
          process.stderr.write(`browser trace cleanup failed: ${error.message}\n`);
        });
      }
    }
  },
);
