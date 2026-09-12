# Apify fallback for `dd-web-scraper`

The local scraper remains the primary and policy-enforcing execution path. The public listener is a small gateway that first forwards every request to the loopback-bound existing service. Apify is considered only after that local request genuinely fails with an eligible server-side execution error.

## Why this shape

This borrows the useful parts of an Actor-style scraping platform without turning an external provider into an access-control bypass or an unbounded spend path:

1. Validate/authenticate/enforce robots and target policy locally.
2. Attempt the requested local strategy.
3. Classify the failure.
4. For a narrow recoverable class, optionally run one Apify Website Content Crawler Actor call.
5. Normalize the first dataset item into the existing response envelope and attach explicit fallback provenance.
6. If Apify also fails, return the original local failure rather than hiding it.

The fallback Actor is hard-bounded to `maxCrawlDepth: 0`, `maxCrawlPages: 1`, and `maxItems=1`. It is not a site crawler.

## Eligibility

Apify fallback is **off by default**. It requires both `SCRAPER_APIFY_FALLBACK=true` and a token in `APIFY_API_TOKEN` (or the compatibility name `SCRAPER_APIFY_API_TOKEN`).

Only HTTP 5xx failures with a recognized recoverable execution signature are eligible, such as a navigation timeout, browser crash/closure, extraction-worker failure, fetch failure, or Browserless 5xx.

The gateway deliberately refuses external fallback for:

- authentication/authorization failures;
- `robots.txt` failures or overrides;
- scraper network-policy/SSRF failures;
- CAPTCHA/challenge failures;
- requests that ask to solve a CAPTCHA;
- requests with a caller-selected proxy;
- contact/phone/email extraction requests;
- selector-shaped extraction requests.

The last two exclusions preserve response semantics: until Apify output is fed back through the same local extraction worker, the gateway must not pretend it reproduced local selector/contact extraction.

## Cost and resource bounds

| Environment variable | Default | Purpose |
| --- | ---: | --- |
| `SCRAPER_APIFY_FALLBACK` | `false` | Master opt-in switch. |
| `SCRAPER_APIFY_ACTOR` | `apify~website-content-crawler` | Actor identifier. |
| `SCRAPER_APIFY_API_BASE` | `https://api.apify.com` | API base. |
| `SCRAPER_APIFY_TIMEOUT_MS` | `90000` | External timeout; hard-capped at 300000 ms. |
| `SCRAPER_APIFY_MAX_CONCURRENT` | `2` | Maximum simultaneous paid fallback calls per pod. |
| `SCRAPER_APIFY_MAX_PER_MINUTE` | `20` | Maximum fallback starts per minute per pod. |
| `SCRAPER_APIFY_MAX_RESPONSE_BYTES` | `2000000` | Maximum accepted dataset response; hard-capped at 10 MB. |
| `APIFY_API_TOKEN` | none | Secret; intentionally not declared as a CLI flag. |

The token is sent in the `Authorization: Bearer` header, never in a query parameter, log field, fallback response, or `.cli-flags.toml`.

## `flags-2-env`

`.cli-flags.toml` declares the non-secret runtime surface. The image pins `@oresoftware/f2e@0.3.0`, audits the TOML during the Docker build, and starts through `dist/entrypoint.js`. The entrypoint asks `flags2env` to resolve the CLI/env/dotenv precedence, then starts `dist/gateway.js` with that resolved environment.

Examples:

```bash
node dist/entrypoint.js --apify-fallback --apify-max-concurrent=1
node dist/entrypoint.js --no-apify-fallback --max-concurrent=4
node dist/entrypoint.js --help
```

Secrets remain ordinary process/Kubernetes secrets rather than command-line flags.

## Deployment

Do not enable the fallback until `APIFY_API_TOKEN` is injected from the deployment's secret manager. A suitable Kubernetes wiring is an optional `secretKeyRef` from `dd-agent-secrets`, followed by `SCRAPER_APIFY_FALLBACK=true` only in the environment where the budget and target policy have been reviewed.

Keep the existing egress NetworkPolicy, robots defaults, private-network blocking, request authentication, and per-origin delay unchanged. External fallback is an availability feature, not a reason to relax those controls.
