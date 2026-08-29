# dd-web-scraper-service

Long-running Fastify service for scraping from the remote Kubernetes runtime.

## Safe and ethical use

Scraping public or operator-authorized data is a safe, ethical, and legitimate
automation technique when callers respect site terms and `robots.txt`, identify
the automation where appropriate, rate-limit it, minimize retained data, and
handle personal information responsibly. Playwright and Puppeteer are provided
for exactly those compliant browser workflows.

This service is not an access-control bypass. Do not use it to evade login,
paywall, CAPTCHA, blocking, or opt-out controls without the target owner's
written authorization. Private/cluster/cloud-metadata targets stay blocked,
and the deployment's egress policy is the final SSRF backstop.

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

## Business contact extraction

The service can pull business phone numbers and email addresses out of a page as
structured fields instead of leaving callers to regex the `text` blob.

This is **opt-in**, because contact details are PII and the collection rule is
"only what the job needs" (see `AGENTS.md`). Request flags:

- `includeContacts` — turn on both phones and emails
- `includePhones` / `includeEmails` — granular, and they override `includeContacts`
- `contactRegion` — ISO 3166-1 alpha-2 region used to normalize local numbers to
  E.164 (defaults to `SCRAPER_CONTACT_REGION`, itself defaulting to `US`)
- `maxPhones` / `maxEmails` — per-request caps, clamped by `SCRAPER_MAX_PHONES`
  and `SCRAPER_MAX_EMAILS` (both default `50`)

Results land on `extraction.contacts`:

```jsonc
{
  "phones": [
    {
      "raw": "(628) 555-0100",
      "e164": "+16285550100",        // ready for Postgres / HubSpot
      "national": "(628) 555-0100",
      "extension": "3140",           // when the page advertises one
      "sources": ["tel-href", "text"],
      "confidence": 1
    }
  ],
  "emails": [{ "address": "sales@acme.test", "sources": ["mailto-href"], "confidence": 1 }]
}
```

Numbers and addresses are gathered from `tel:`/`mailto:` hrefs, schema.org
JSON-LD (`telephone`, `email`, `faxNumber`), `<meta>` tags, and visible text —
then deduplicated by E.164, with `sources` merged and `confidence` set to the
best source that saw it (`tel:`/`mailto:` = 1.0, JSON-LD = 0.95, meta = 0.9,
free text = 0.6). Use `confidence` to decide what syncs automatically and what
gets queued for review.

Precision matters more than recall here, since false positives pollute the CRM.
So the extractor drops runs glued to `#`/currency symbols (order numbers, SKUs,
prices), repeated-digit and sequential placeholders, and — for free text with no
country code — unformatted digit runs that aren't NANP-shaped. Inline `<script>`
and `<style>` bodies are excluded from the markup scan so vendor config values
don't surface as contacts. HTML entities are decoded first, so `info&#64;acme.test`
is still found.

Every emitted `e164` is checked for structural validity before it is returned:
country codes never start with `0`, the number is 8–15 digits, and `+1` numbers
must satisfy the North American Numbering Plan (area code and exchange both start
2–9). So `+1 111 555 0100` and `+0…` are dropped rather than synced. These are
hard, universal rules, so the check never removes a genuine business line; it is
not a substitute for a full validation library (e.g. `libphonenumber`) if you
later need per-country national-format validation.

Scanning is ReDoS-safe: the email/phone patterns use bounded quantifiers, so a
hostile page (the scan runs over untrusted, attacker-controlled text up to
`MAX_SCAN_CHARS`) cannot trigger catastrophic backtracking in a parser worker.

Because contact data is frequently injected client-side (click-to-call widgets,
"reveal number" buttons), the `playwright` and `puppeteer` strategies find
numbers the static parsers cannot — they extract from the rendered DOM. Prefer a
browser strategy when a target's contact details don't appear in the static HTML.

Extracted values are never logged; only counts appear in telemetry.

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

## Responsible scraping policy

Browser automation and scraping are safe, ethical engineering techniques when
used on public resources or systems the operator owns or is authorized to
access, within the target's terms and applicable law. This project policy is
not blanket permission or legal advice. Prefer an official API when practical,
collect only what the job needs, avoid personal/sensitive data, and never use
the service to bypass unauthorized authentication, paywalls, or rate limits.

The service identifies itself with `SCRAPER_USER_AGENT`, checks `robots.txt` by
default (`SCRAPER_RESPECT_ROBOTS=true`), honors a declared crawl delay, and
spaces top-level requests to each origin by at least
`SCRAPER_MIN_ORIGIN_DELAY_MS` (default 1 second). A request can set
`"respectRobots": false` only when an operator has deliberately enabled
`SCRAPER_ALLOW_ROBOTS_OVERRIDE=true` for an owned or contractually authorized
target. Robots documents are fetched through the same SSRF-safe/proxy path and
cached for `SCRAPER_ROBOTS_CACHE_TTL_MS`. Prometheus exposes checks, denials,
and overrides as `dd_web_scraper_robots_*` counters.

## CAPTCHA solving orchestration

For every scrape the fetched page is scanned for a challenge — reCAPTCHA v2/v3,
hCaptcha, Cloudflare Turnstile, and Cloudflare interstitial ("Just a moment") —
and the result is reported as `response.captcha` (`detected`, `type`, `sitekey`,
`signals`). Detection is on by default (`SCRAPER_DETECT_CAPTCHAS`); a request may
override with `"detectCaptcha": false`.

On the browser strategies (`playwright`/`puppeteer`) the service can also solve
a challenge on an owned or explicitly authorized test system. Solving requires
all three controls: `SCRAPER_ALLOW_CAPTCHA_SOLVING=true`,
`SCRAPER_CAPTCHA_AUTOSOLVE=true` (or request `"solveCaptcha": true`), and a
solver API key. The service submits the sitekey, injects the returned token,
fires the widget callback, and continues to extraction. `solved: true` means a
token was obtained and applied. Static strategies report detection only.

The solver client speaks the 2captcha `in.php`/`res.php` protocol, which CapSolver,
CapMonster, and Anti-Captcha expose a compatible surface for, so the vendor is
swappable via `SCRAPER_CAPTCHA_PROVIDER_URL`. Config: `SCRAPER_CAPTCHA_API_KEY`
(enables solving), `SCRAPER_CAPTCHA_POLL_INTERVAL_MS`, `SCRAPER_CAPTCHA_TIMEOUT_MS`,
`SCRAPER_CAPTCHA_MAX_ATTEMPTS`, `SCRAPER_CAPTCHA_MAX_CONCURRENT`. Prometheus exposes
`dd_web_scraper_captcha_total` labelled by `event` (`detected`/`solved`/`failed`)
and `type`.

### Hardening notes

- **Cost amplification.** A solve holds the in-flight slot and the browser page
  for up to `SCRAPER_CAPTCHA_TIMEOUT_MS` (longer than the request timeout) and
  costs money per solve. A hostile target can serve a fake sitekey to trigger
  this. Solver use and auto-solve are therefore both off by default, and
  `SCRAPER_CAPTCHA_MAX_CONCURRENT` caps simultaneous solves — excess challenges are reported as detected-only
  (`captcha.error = "captcha solver concurrency limit reached"`) rather than
  queued. Keep auto-solve scoped to trusted target sets.
- **Per-request proxy.** `"proxy"` lets an authenticated caller route through an
  arbitrary proxy; the proxy host is re-resolved and rejected if it lands on a
  private/cluster address (unless `SCRAPER_ALLOW_PRIVATE_NETWORKS=true`). Set
  `SCRAPER_ALLOW_REQUEST_PROXY=false` to forbid it entirely and pin egress to the
  operator-configured pool.
- **Proxy health.** A pooled proxy whose response is `407`/`403`/`429` or carries
  a detected challenge is treated as unhealthy and cooled out of rotation; the
  next request tries a different egress IP.
- **Detection is linear.** CAPTCHA detection runs on every scrape over HTML up to
  `SCRAPER_MAX_HTML_CHARS`; each pattern is gated behind a substring check so a
  large non-matching document can't drive quadratic regex backtracking.
- **DNS-rebinding.** Native-fetch strategies connect through an undici agent whose
  DNS lookup re-checks the resolved address against the private-network policy
  *at connect time*, so a host that passes the pre-flight check but rebinds to a
  private/link-local/cloud-metadata address (e.g. `169.254.169.254`) is still
  refused. (Browser strategies retain the per-subresource pre-flight route guard;
  Chromium-level IP pinning is out of scope.)
- **Input bounds.** `userAgent` and outbound header values reject control
  characters (no header smuggling through the browser strategies, which bypass the
  fetch header guard); `selectors` is capped at 50 entries; request bodies are
  capped at 1 MiB.
- **Parser isolation.** HTML extraction runs in `worker_threads` with memory and
  time limits. The DOM parsers are inert by construction: jsdom and linkedom are
  invoked without script execution or subresource loading, so untrusted HTML
  cannot run JS or fetch URLs out of the parser.
- **Network-layer egress lockdown.** `dd-web-scraper.networkpolicy.yaml` is the
  authoritative backstop: it denies all ingress except the gateway and the
  observability scrapers, and restricts egress to cluster DNS plus the public
  internet on any port *except* the private/link-local/cloud-metadata ranges. This
  is what protects the browser strategies — which the app cannot IP-pin — against
  DNS-rebinding to internal services or `169.254.169.254`, since the kernel drops
  the packet regardless of what Chromium resolves. The in-app `isPrivateIp` guard
  and the connect-time DNS check are the first two layers; this is the third.
- **Solver response is bounded.** The CAPTCHA solver client caps the provider
  response body (64 KiB) so a compromised or misconfigured `SCRAPER_CAPTCHA_PROVIDER_URL`
  can't stream an unbounded body into memory.
- **Bounded DNS resolution.** The pre-flight SSRF checks resolve the target and
  proxy hostnames before any fetch timeout applies. That lookup is raced against
  `SCRAPER_DNS_TIMEOUT_MS` (default 5 s) so a hostname whose authoritative server
  deliberately stalls can't pin an in-flight slot for the full `getaddrinfo`
  timeout — bounding an otherwise un-timed phase of the request.

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
