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

## Proxy rotation

Outbound requests can rotate across a pool of proxies. The pool is configured with
`SCRAPER_PROXIES` — a comma/newline/whitespace-separated list of proxy URLs
(`http`, `https`, `socks4`, `socks5`; bare `host:port` and `user:pass@host:port`
are assumed `http`). An empty list means direct egress.

`SCRAPER_PROXY_ROTATION` selects the strategy:

- `sticky` (default): one egress proxy per target host, so a host keeps the same
  IP across requests and sessions don't break mid-crawl.
- `round-robin`: walk the pool in order.
- `random`: pick uniformly at random.

A proxy that fails a request is put on a `SCRAPER_PROXY_COOLDOWN_MS` cooldown and
skipped until it expires (if every proxy is cooling down, the pool degrades to
reusing one rather than dropping the scrape). Proxy applies to `native-fetch`,
`cheerio`, `jsdom`, `linkedom` (via an undici `ProxyAgent`, HTTP/HTTPS only),
`playwright`, and `puppeteer` (which also accept SOCKS). `browserless` manages its
own egress and is left untouched.

Per request:

- `"proxy": "http://user:pass@host:port"` forces a specific proxy (gated by
  `SCRAPER_ALLOW_REQUEST_PROXY`, default on; the host is re-checked against the
  same private-network policy as targets).
- `"useProxy": false` bypasses the pool for that request.

The chosen proxy is reported back as `response.proxy` (label has credentials
stripped). Prometheus exposes `dd_web_scraper_proxy_pool_size`,
`dd_web_scraper_proxy_pool_available`, `dd_web_scraper_proxy_selections_total`,
and `dd_web_scraper_proxy_failures_total`.

## CAPTCHA solving orchestration

For every scrape the fetched page is scanned for a challenge — reCAPTCHA v2/v3,
hCaptcha, Cloudflare Turnstile, and Cloudflare interstitial ("Just a moment") —
and the result is reported as `response.captcha` (`detected`, `type`, `sitekey`,
`signals`). Detection is on by default (`SCRAPER_DETECT_CAPTCHAS`); a request may
override with `"detectCaptcha": false`.

On the browser strategies (`playwright`/`puppeteer`) the service can also solve
the challenge: when `SCRAPER_CAPTCHA_AUTOSOLVE=true` (or a request sets
`"solveCaptcha": true`) and a solver API key is present, it submits the sitekey to
the provider, injects the returned token into the page's response field, fires the
widget callback, and continues to extraction. `solved: true` means a token was
obtained and applied. Static strategies report detection only — they have no page
to inject into, so retry through a browser strategy.

The solver client speaks the 2captcha `in.php`/`res.php` protocol, which CapSolver,
CapMonster, and Anti-Captcha expose a compatible surface for, so the vendor is
swappable via `SCRAPER_CAPTCHA_PROVIDER_URL`. Config: `SCRAPER_CAPTCHA_API_KEY`
(enables solving), `SCRAPER_CAPTCHA_POLL_INTERVAL_MS`, `SCRAPER_CAPTCHA_TIMEOUT_MS`,
`SCRAPER_CAPTCHA_MAX_ATTEMPTS`. Prometheus exposes `dd_web_scraper_captcha_total`
labelled by `event` (`detected`/`solved`/`failed`) and `type`.

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
