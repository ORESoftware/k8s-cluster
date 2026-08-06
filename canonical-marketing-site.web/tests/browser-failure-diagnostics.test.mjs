import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  diagnosticUrl,
  writeBrowserFailureDiagnostics,
} from "./browser-failure-diagnostics.mjs";

function fakePage() {
  return {
    url: () => "https://canonical.cloud/final?session=private#fragment",
    screenshot: async ({ path, fullPage }) => {
      assert.equal(fullPage, true);
      await writeFile(path, "fake-png");
    },
    content: async () => `<html>${"x".repeat(1_000_050)}</html>`,
  };
}

function fakeContext({ cookies = [] } = {}) {
  return {
    cookies: async () => cookies,
    tracing: {
      stop: async (options) => {
        if (options?.path) await writeFile(options.path, "fake-trace");
      },
    },
  };
}

function fakeResponse(headers = {}) {
  return {
    status: () => 503,
    allHeaders: async () => ({
      "content-security-policy": "default-src 'self'",
      "content-type": "text/html; charset=utf-8",
      server: "should-not-be-captured",
      ...headers,
    }),
  };
}

test("diagnostic URLs remove credentials, queries, and fragments", () => {
  assert.equal(
    diagnosticUrl("https://user:password@example.test/path?token=secret#fragment"),
    "https://example.test/path",
  );
  assert.equal(diagnosticUrl("not a URL"), "<invalid-url>");
});

test("a failed browser assertion creates a bounded, filtered evidence bundle", async (t) => {
  const artifactDirectory = await mkdtemp(join(tmpdir(), "canonical-browser-evidence-"));
  t.after(() => rm(artifactDirectory, { recursive: true, force: true }));

  const result = await writeBrowserFailureDiagnostics({
    artifactDirectory,
    context: fakeContext(),
    page: fakePage(),
    response: fakeResponse(),
    targetUrl: "https://canonical.cloud/?token=secret",
    error: new Error("security policy mismatch"),
    externalRequests: ["https://third.invalid/script.js?token=secret#fragment"],
    failedRequests: [
      {
        method: "GET",
        url: "https://third.invalid/fail?credential=secret",
        errorText: "net::ERR_FAILED",
      },
    ],
    pageErrors: ["page failed"],
    consoleMessages: [{ type: "error", text: "console failed" }],
  });

  assert.deepEqual(result, { tracingStopped: true, written: true });
  assert.deepEqual((await readdir(artifactDirectory)).sort(), [
    "diagnostics.json",
    "page.html",
    "page.png",
    "trace.zip",
  ]);

  const diagnosticText = await readFile(
    join(artifactDirectory, "diagnostics.json"),
    "utf8",
  );
  const diagnostic = JSON.parse(diagnosticText);
  assert.equal(diagnostic.targetUrl, "https://canonical.cloud/");
  assert.equal(diagnostic.finalUrl, "https://canonical.cloud/final");
  assert.equal(diagnostic.responseStatus, 503);
  assert.equal(diagnostic.securityHeaders.server, undefined);
  assert.equal(
    diagnostic.securityHeaders["content-security-policy"],
    "default-src 'self'",
  );
  assert.equal(diagnostic.externalRequests[0], "https://third.invalid/script.js");
  assert.equal(diagnostic.failedRequests[0].url, "https://third.invalid/fail");
  assert.equal(diagnostic.evidence.htmlTruncated, true);
  assert.equal(diagnostic.evidence.traceWritten, true);
  assert.doesNotMatch(diagnosticText, /token=secret|credential=secret|password/);

  assert.equal((await stat(join(artifactDirectory, "page.html"))).size, 1_000_000);
});

test("a context with cookies discards its trace rather than persisting it", async (t) => {
  const artifactDirectory = await mkdtemp(join(tmpdir(), "canonical-browser-evidence-"));
  t.after(() => rm(artifactDirectory, { recursive: true, force: true }));

  await writeBrowserFailureDiagnostics({
    artifactDirectory,
    context: fakeContext({ cookies: [{ name: "unexpected" }] }),
    page: fakePage(),
    response: fakeResponse({ "set-cookie": "unexpected=value" }),
    targetUrl: "https://canonical.cloud/",
    error: new Error("unexpected cookie"),
  });

  const files = await readdir(artifactDirectory);
  assert.equal(files.includes("trace.zip"), false);
  const diagnostic = JSON.parse(
    await readFile(join(artifactDirectory, "diagnostics.json"), "utf8"),
  );
  assert.equal(diagnostic.securityHeaders["set-cookie"], undefined);
  assert.equal(diagnostic.evidence.traceWritten, false);
  assert.match(diagnostic.evidence.traceOmittedReason, /cookies|Set-Cookie/);
});
