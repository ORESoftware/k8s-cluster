// Repo-local boot recipe for the marketing-site browser E2E.
//
// The genuinely-shared pieces (Chrome discovery + the server lifecycle) come
// from @fiducia/test-config; only the Astro-specific base-path and boot
// arguments live here, next to the app they boot. Specs stay in this repo.
import assert from "node:assert/strict";
import { startServer } from "@fiducia/test-config/harness";

export { chromeExecutablePath, launchOptions } from "@fiducia/test-config/harness";

function normalizePublicBase(value) {
  const trimmed = String(value).trim();
  if (trimmed === "" || trimmed === "/") return "";
  const normalized = `/${trimmed.replace(/^\/+|\/+$/g, "")}`;
  assert.match(
    normalized,
    /^\/[A-Za-z0-9/_-]+$/,
    `PUBLIC_BASE contains unsupported characters: ${value}`,
  );
  return normalized;
}

export const publicBase = normalizePublicBase(process.env.PUBLIC_BASE ?? "/fiducia");

// Return one absolute path under the configured deployment base. The empty
// suffix is the site root; callers never concatenate path prefixes manually.
export function sitePath(suffix = "") {
  const cleanSuffix = String(suffix).replace(/^\/+/, "");
  return cleanSuffix ? `${publicBase}/${cleanSuffix}` : `${publicBase}/` || "/";
}

// Boots `astro preview` on an ephemeral port and waits at the exact build base.
// PUBLIC_BASE is consumed by astro.config.mjs during `npm run build`; this
// harness uses the same value for readiness and browser navigation so root and
// `/fiducia` builds exercise the same contract instead of hard-coding one mode.
// Set FIDUCIA_SITE_TEST_URL to run the suite against an already-running site.
export function startSite() {
  return startServer({
    command: "npm",
    args: ["run", "preview", "--"],
    cwd: new URL("..", import.meta.url).pathname,
    portArgs: (port) => ["--port", String(port), "--host", "127.0.0.1"],
    readyPath: sitePath(),
    reuseUrlEnv: "FIDUCIA_SITE_TEST_URL",
    startupTimeoutMs: 45000,
  });
}
