# Local-first scraping with Apify fallback

`dd-web-scraper` keeps the existing in-cluster scraper as the primary execution path. A small supervisor runs on the public service port and starts the existing Fastify scraper core on loopback. Ordinary routes are proxied to the core unchanged.

For `POST /scrape`, the supervisor may call Apify only after the local core returned a **classified retriable 5xx**. It never falls back for validation/policy responses, robots denials, CAPTCHA/challenge errors, authentication/authorization failures, private-network/metadata policy failures, or an unavailable local core. By default, an explicitly selected strategy such as `playwright` also does not fall back; `auto` is the intended fallback mode.

This is a reliability fallback, not an access-control bypass. Apify receives only the target URL and extraction shape needed for the request. Caller cookies, Authorization headers, proxy credentials, URL credentials, and other outbound headers are never forwarded to Apify.

## Apify bounds

The default Actor is `apify/web-scraper`. Each fallback run is constrained to one start URL, an empty `linkSelector`, one page/result, and Actor concurrency 1. `respectRobotsTxtFile=true` is forced even when a local request-level override was authorized. Page retries default to 2, pod-level external fallback concurrency defaults to 1, the fallback waits at least 1 second after the local attempt, and provider failures trip a 60-second cooldown.

The synchronous Actor call also sends `maxItems=1`, a configurable `maxTotalChargeUsd` (USD 0.25 by default), a bounded run/client timeout, and a bounded response-body limit. Provider debug/browser logs are disabled.

The Apify token is sent only as `Authorization: Bearer …`; it is never placed in the request URL or emitted by supervisor logs/status/metrics.

## flags-2-env

The service root contains `.cli-flags.toml`, the `flags-2-env` contract for non-secret runtime settings. The canonical package is `flags-2-env/flags-2-env` / `@oresoftware/f2e`.

From this service directory:

```bash
pnpm run config:audit
```

Secret-bearing values are deliberately absent from CLI flags so they do not land in shell history: `APIFY_TOKEN`, `SERVER_AUTH_SECRET`, `BROWSERLESS_TOKEN`, `BROWSERLESS_CONTENT_URL`, `SCRAPER_CAPTCHA_API_KEY`, `SCRAPER_PROXIES`, and `BROWSER_AGENT_SECRETS_FILE` remain secret-store/environment inputs.

## Fallback variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `SCRAPER_CORE_PORT` | `18097` | loopback port for the existing Fastify core |
| `SCRAPER_CORE_STARTUP_TIMEOUT_MS` | `30000` | fail startup if the core is unhealthy |
| `SCRAPER_SUPERVISOR_MAX_BODY_BYTES` | `1048576` | replay buffer cap for `/scrape` |
| `SCRAPER_SUPERVISOR_MAX_CORE_RESPONSE_BYTES` | `4194304` | local `/scrape` response cap |
| `APIFY_FALLBACK_ENABLED` | `false` in code | master external-fallback switch |
| `APIFY_TOKEN` | unset | secret API token; required for any external call |
| `APIFY_ACTOR_ID` | `apify/web-scraper` | Actor used for the one-page fallback |
| `APIFY_API_BASE_URL` | `https://api.apify.com/v2` | HTTPS API base URL |
| `APIFY_FALLBACK_TIMEOUT_MS` | `90000` | request/run timeout |
| `APIFY_MAX_REQUEST_RETRIES` | `2` | page retries inside the Actor |
| `APIFY_FALLBACK_MAX_CONCURRENT` | `1` | pod-level external concurrency cap |
| `APIFY_FALLBACK_MIN_DELAY_MS` | `1000` | minimum delay before remote fallback |
| `APIFY_FALLBACK_FAILURE_COOLDOWN_MS` | `60000` | cooldown after provider failures |
| `APIFY_MAX_TOTAL_CHARGE_USD` | `0.25` | per-run billing ceiling |
| `APIFY_FALLBACK_FOR_EXPLICIT_STRATEGY` | `false` | permit fallback after explicit local strategies |
| `APIFY_MAX_RESPONSE_BYTES` | `2000000` | provider-response buffer cap |

## Observability

`GET /status` adds a non-secret `externalFallback` descriptor. `GET /fallback/status` returns provider configuration without the token. Existing `/metrics` output is augmented with bounded-label counters for fallback attempts, successes, failures, in-flight calls, and skipped failures by classifier reason.

A successful external result uses `strategy: "apify"`, `provider: "apify"`, and a `fallback` object describing the local status/strategy and timing. Opt-in lead/contact extraction reuses the existing `contacts.ts` normalization logic.