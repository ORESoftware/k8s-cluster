import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const MAX_ENTRIES = 100;
const MAX_ENTRY_TEXT = 2_000;
const MAX_ERROR_STACK = 8_000;
const MAX_HTML_CHARS = 1_000_000;

const SECURITY_HEADER_NAMES = [
  "cache-control",
  "content-security-policy",
  "content-type",
  "cross-origin-opener-policy",
  "permissions-policy",
  "referrer-policy",
  "strict-transport-security",
  "x-content-type-options",
  "x-frame-options",
  "x-request-id",
];

function boundedText(value, maximum = MAX_ENTRY_TEXT) {
  const text = String(value ?? "");
  return text.length <= maximum ? text : `${text.slice(0, maximum)}…[truncated]`;
}

function boundedEntries(entries) {
  return entries.slice(0, MAX_ENTRIES).map((entry) => {
    if (typeof entry === "string") return boundedText(entry);
    return Object.fromEntries(
      Object.entries(entry).map(([key, value]) => [key, boundedText(value)]),
    );
  });
}

export function diagnosticUrl(value) {
  try {
    const url = new URL(value);
    url.username = "";
    url.password = "";
    url.search = "";
    url.hash = "";
    return url.toString();
  } catch {
    return "<invalid-url>";
  }
}

function securityHeaders(headers) {
  return Object.fromEntries(
    SECURITY_HEADER_NAMES.filter((name) => headers[name] !== undefined).map((name) => [
      name,
      boundedText(headers[name]),
    ]),
  );
}

async function captureFile(name, operation, artifactErrors) {
  try {
    await operation();
    return true;
  } catch (error) {
    artifactErrors.push(`${name}: ${boundedText(error?.message ?? error)}`);
    return false;
  }
}

/**
 * Persist bounded, public-surface browser evidence after an assertion failure.
 *
 * The bundle deliberately excludes cookies, arbitrary request/response headers,
 * request bodies, environment variables, and page storage. A Playwright trace
 * is retained only when the fresh browser context has no cookies and the main
 * response did not advertise Set-Cookie; otherwise the trace is discarded.
 */
export async function writeBrowserFailureDiagnostics({
  artifactDirectory,
  context,
  page,
  response,
  targetUrl,
  error,
  externalRequests = [],
  failedRequests = [],
  pageErrors = [],
  consoleMessages = [],
}) {
  if (!artifactDirectory) {
    return { tracingStopped: false, written: false };
  }

  await mkdir(artifactDirectory, { recursive: true });
  const artifactErrors = [];
  const allHeaders = response
    ? await response.allHeaders().catch((headerError) => {
        artifactErrors.push(`headers: ${boundedText(headerError?.message ?? headerError)}`);
        return {};
      })
    : {};
  const cookies = await context.cookies().catch((cookieError) => {
    artifactErrors.push(`cookie inventory: ${boundedText(cookieError?.message ?? cookieError)}`);
    return ["unknown"];
  });

  const screenshotWritten = await captureFile(
    "screenshot",
    () =>
      page.screenshot({
        path: join(artifactDirectory, "page.png"),
        fullPage: true,
      }),
    artifactErrors,
  );

  let htmlTruncated = false;
  const htmlWritten = await captureFile(
    "rendered HTML",
    async () => {
      const html = await page.content();
      htmlTruncated = html.length > MAX_HTML_CHARS;
      await writeFile(
        join(artifactDirectory, "page.html"),
        html.slice(0, MAX_HTML_CHARS),
        "utf8",
      );
    },
    artifactErrors,
  );

  const traceSafe = cookies.length === 0 && allHeaders["set-cookie"] === undefined;
  let traceWritten = false;
  let tracingStopped = false;
  if (traceSafe) {
    traceWritten = await captureFile(
      "Playwright trace",
      async () => {
        await context.tracing.stop({ path: join(artifactDirectory, "trace.zip") });
        tracingStopped = true;
      },
      artifactErrors,
    );
  } else {
    await captureFile(
      "discard Playwright trace",
      async () => {
        await context.tracing.stop();
        tracingStopped = true;
      },
      artifactErrors,
    );
  }

  const normalizedError = error instanceof Error ? error : new Error(String(error));
  const diagnostic = {
    capturedAt: new Date().toISOString(),
    targetUrl: diagnosticUrl(targetUrl),
    finalUrl: diagnosticUrl(page.url()),
    responseStatus: response?.status() ?? null,
    securityHeaders: securityHeaders(allHeaders),
    error: {
      name: boundedText(normalizedError.name),
      message: boundedText(normalizedError.message),
      stack: boundedText(normalizedError.stack, MAX_ERROR_STACK),
    },
    externalRequests: boundedEntries(
      externalRequests.map((url) => diagnosticUrl(url)),
    ),
    failedRequests: boundedEntries(
      failedRequests.map((request) => ({
        ...request,
        url: diagnosticUrl(request.url),
      })),
    ),
    pageErrors: boundedEntries(pageErrors),
    consoleMessages: boundedEntries(consoleMessages),
    evidence: {
      screenshotWritten,
      htmlWritten,
      htmlTruncated,
      traceWritten,
      traceOmittedReason: traceSafe
        ? null
        : "fresh context contained cookies or the response advertised Set-Cookie",
      artifactErrors: boundedEntries(artifactErrors),
    },
  };

  await writeFile(
    join(artifactDirectory, "diagnostics.json"),
    `${JSON.stringify(diagnostic, null, 2)}\n`,
    "utf8",
  );

  return { tracingStopped, written: true };
}
