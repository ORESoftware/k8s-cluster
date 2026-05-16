# dd-web-scraper-service

Long-running Fastify service for scraping from the remote Kubernetes runtime.

## Framework choice

This service uses Fastify instead of Nest because the current server state is small and local:
strategy selection, browser process reuse, in-flight request accounting, and Prometheus counters.
Nest would be reasonable later if scraping turns into a larger workflow domain with persistent job
state, scheduled crawls, injectable stores, or multiple coordinated modules.

## Worker model

Browser rendering already happens outside the Node.js event loop in Chromium child processes. The
remaining CPU-heavy work is HTML extraction, so the service runs every parser strategy through
`worker_threads` in `src/extraction-worker.ts`.

`SCRAPER_PARSER_WORKERS` caps concurrent parser workers per pod. The default is one fewer than the
available CPU count, capped at four; Kubernetes pins it to `2` today so the pod leaves headroom for
Fastify and browser orchestration.

## Strategies

- `auto`: chooses `playwright` for JavaScript rendering, `cheerio` for selector extraction, and
  `native-fetch` for plain fetches.
- `native-fetch`: Node runtime `fetch`; title/text extraction runs in a parser worker.
- `cheerio`: static HTML fetch plus jQuery-style selector extraction in a parser worker.
- `jsdom`: static HTML fetch plus browser-like DOM APIs in a parser worker.
- `linkedom`: static HTML fetch plus a lightweight DOM parser in a parser worker. This is the extra
  strategy beyond the requested list.
- `playwright`: pooled Chromium browser for JavaScript-rendered pages.
- `puppeteer`: pooled Chromium browser through Puppeteer.
- `browserless`: Browserless Content API, configured with `BROWSERLESS_TOKEN` or a
  `BROWSERLESS_CONTENT_URL` that already includes a token.

## Browser mode and failure screenshots

Playwright and Puppeteer launch Chromium with `SCRAPER_BROWSER_HEADLESS=true` by default. The
Kubernetes deployment uses the Playwright browser image without an X server, so headed mode should
only be enabled if the container runtime also provides a display.

For browser strategies, failed navigation or extraction responses can include a bounded JPEG
`failureScreenshot` payload. This is enabled by `SCRAPER_CAPTURE_FAILURE_SCREENSHOTS=true`, uses
`SCRAPER_FAILURE_SCREENSHOT_QUALITY=65`, and omits the base64 payload when the screenshot exceeds
`SCRAPER_FAILURE_SCREENSHOT_MAX_BYTES`.

## API

Protected endpoint:

```bash
curl -X POST http://localhost:8097/scrape \
  -H "content-type: application/json" \
  -H "x-server-auth: $SERVER_AUTH_SECRET" \
  -d '{
    "url": "https://example.com",
    "strategy": "auto",
    "selector": "h1",
    "includeLinks": true
  }'
```

Public diagnostics:

- `GET /healthz`
- `GET /metrics`
- `GET /status`
- `GET /strategies`

The gateway also exposes those under `/scrape/healthz`, `/scrape/metrics`, `/scrape/status`, and
`/scrape/strategies`.

Private and cluster-local targets are blocked by default. Set `SCRAPER_ALLOW_PRIVATE_NETWORKS=true`
only for a tightly controlled internal use case.

Security defaults:

- `SERVER_AUTH_SECRET` is required unless `SCRAPER_ALLOW_UNAUTHENTICATED=true`.
- Redirect targets and browser subresource requests are rechecked against the same network policy.
- URL credentials and sensitive outbound headers such as `Authorization` and `Cookie` are blocked
  unless explicitly enabled with `SCRAPER_ALLOW_URL_CREDENTIALS=true` or
  `SCRAPER_ALLOW_SENSITIVE_HEADERS=true`.
- Parser workers use `resourceLimits`; Kubernetes currently sets
  `SCRAPER_PARSER_WORKER_MEMORY_MB=128`.
