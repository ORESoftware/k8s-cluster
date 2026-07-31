# dd-browser-test-server

On-demand browser automation runner. A single Fastify HTTP service that drives
real Chromium with three back-ends — Playwright, Puppeteer, and Selenium —
behind one declarative scenario API.

This is sibling infrastructure to `dd-web-scraper`:

| Service              | Purpose                                                | Port |
| -------------------- | ------------------------------------------------------ | ---- |
| `dd-web-scraper`     | One-shot HTML scraping (fetch, cheerio, jsdom, browser) | 8097 |
| `dd-browser-test-server` | End-to-end UI scenarios (click, fill, screenshot, ...) | 8104 |

The scraper is optimised for "give me a parsed page once". This service is
optimised for "drive a browser through a real interaction sequence and tell me
what happened". The two services intentionally do *not* share runtime state —
they share only the Playwright Noble base image so the bundled Chromium binary
backs all four drivers (cheerio is HTML-only).

## Why all three drivers in one service?

So tests can pick the driver that matches the failure mode they want to
reproduce, without spinning up three deployments:

- **Playwright** — first-class auto-waiting; best default for happy-path UI
  smokes against `/agents/threads`, the gleam fanout, and the dev-server UI.
- **Puppeteer** — closer to raw CDP; useful when reproducing problems we see
  in headless Chrome that auto-waiting masks (e.g. flaky network-idle waits).
- **Selenium** — W3C WebDriver behaviour; needed for parity testing with
  Selenium-based external suites and for the very small number of features
  that only Selenium implements (e.g. Print Preview automation).

All three drivers reuse Playwright's bundled Chromium via
`playwright.chromium.executablePath()`. Selenium uses Selenium Manager (4.x)
to resolve a matching `chromedriver` binary on first launch.

## API

Authenticated endpoint (header `x-server-auth: $SERVER_AUTH_SECRET`):

```bash
curl -X POST http://localhost:8104/run \
  -H "content-type: application/json" \
  -H "x-server-auth: $SERVER_AUTH_SECRET" \
  -d '{
    "tool": "playwright",
    "url": "https://example.com",
    "captureFinalScreenshot": true,
    "steps": [
      { "action": "goto", "url": "https://example.com" },
      { "action": "waitForSelector", "selector": "h1" },
      { "action": "extractText", "selector": "h1", "name": "headline" }
    ]
  }'
```

Public diagnostics:

- `GET /healthz`
- `GET /metrics`
- `GET /status`
- `GET /tools`

The gateway also exposes those under `/browser-test/healthz`,
`/browser-test/metrics`, `/browser-test/status`, and `/browser-test/tools`.

### Scenario steps

Each request body has a top-level `steps` array. The supported actions are:

| Action            | Required fields                         | Notes                                          |
| ----------------- | --------------------------------------- | ---------------------------------------------- |
| `goto`            | `url`                                   | Optional `waitUntil` (load/domcontentloaded/networkidle). |
| `click`           | `selector`                              | Optional `nth` for matched-element index.       |
| `fill`            | `selector`, `value`                     | Clears + types into the element.                |
| `select`          | `selector`, `value`                     | Selects an option by value (Selenium falls back to sendKeys). |
| `press`           | `key` (+ optional `selector`)           | Keyboard press; focuses `selector` first when supplied. |
| `waitForSelector` | `selector`                              | Optional `state` (attached/detached/visible/hidden). |
| `waitForUrl`      | `url`                                   | String contains; wrap with `/.../` for regex.   |
| `waitForTimeout`  | `ms`                                    | Hard sleep (max 60s).                           |
| `extractText`     | `selector`                              | Stored in `extracted[name|"text:<sel>"]`.       |
| `extractAttribute`| `selector`, `attribute`                 | Stored in `extracted[name|"attr:<sel>@<attr>"]`. |
| `screenshot`      | (`name`, `fullPage` optional)           | JPEG/PNG; bytes capped by `BROWSER_TEST_MAX_SCREENSHOT_BYTES`. |
| `evaluate`        | `script`                                | **Disabled by default.** Set `BROWSER_TEST_ALLOW_EVALUATE=true`. |

### Response shape

```json
{
  "ok": true,
  "tool": "playwright",
  "requestId": "...",
  "durationMs": 1234,
  "startedAt": "...",
  "finishedAt": "...",
  "finalUrl": "...",
  "finalTitle": "...",
  "steps": [
    { "index": 0, "action": "goto", "status": "ok", "durationMs": 220 }
  ],
  "extracted": { "headline": "Example Domain" },
  "screenshots": [
    { "name": "final", "contentType": "image/jpeg", "base64": "...", "bytes": 31415 }
  ],
  "consoleEntries": [],
  "pageErrors": []
}
```

## Security model

- `SERVER_AUTH_SECRET` is required for `POST /run` unless
  `BROWSER_TEST_ALLOW_UNAUTHENTICATED=true` (intended only for local dev).
- `evaluate` steps are **off by default**. Even with auth, unrestricted
  in-page script execution would let a stolen header POST a `goto` to a
  cluster-internal URL and exfiltrate authenticated content. Operators must
  flip `BROWSER_TEST_ALLOW_EVALUATE=true` when they explicitly want that.
- The service does not validate the target URL against a private-network
  block list — unlike `dd-web-scraper` it is intended to be pointed at the
  cluster's own gateway/services. Treat the gateway auth shape as the
  perimeter, not URL allow-listing.

## Concurrency

`BROWSER_TEST_MAX_CONCURRENT` caps in-flight scenarios per pod (default 2). A
new `POST /run` while at the cap responds `429`. Each scenario runs in a fresh
browser context (Playwright/Puppeteer) or a fresh WebDriver session
(Selenium), so cookies and storage do not leak between runs.

## Operational notes

- **Image:** `mcr.microsoft.com/playwright:v1.56.0-noble` (same as scraper).
- **Port:** 8104.
- **Shared module:** none — this service intentionally does not depend on
  any other internal package so build failures in shared libs cannot break
  on-call's smoke harness.
- **Selenium Manager** (built into selenium-webdriver 4.11+) auto-downloads
  a chromedriver matching the bundled Chromium on first cold start. That
  download is cached under `$XDG_CACHE_HOME` (`/tmp/.cache`) which Kubernetes
  mounts as an `emptyDir`, so each pod recovers a working driver in roughly
  5–10s after start.
