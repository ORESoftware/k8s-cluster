# Local-first scraping with ordered Apify fallback chains

`dd-web-scraper` keeps the existing in-cluster scraper as the primary execution path. A small supervisor runs on the public service port and starts the existing Fastify scraper core on loopback. Ordinary routes are proxied to the core unchanged.

For `POST /scrape`, the supervisor may call Apify only after the local core returned a **classified retriable 5xx**. It never falls back for validation/policy responses, robots denials, CAPTCHA/challenge errors, authentication/authorization failures, private-network/metadata policy failures, or an unavailable local core. By default, an explicitly selected strategy such as `playwright` also does not fall back; `auto` is the intended fallback mode.

This is a reliability fallback, not an access-control bypass. Apify receives only the target URL and extraction shape needed for the request. Caller cookies, Authorization headers, proxy credentials, URL credentials, and other outbound headers are never forwarded to Apify.

## Ordered fallback model

An eligible request follows this shape:

1. local `dd-web-scraper` strategies remain authoritative;
2. select an exact-host or longest matching `*.suffix` Actor route, otherwise use the default Actor chain;
3. execute at most `APIFY_FALLBACK_MAX_ATTEMPTS` Actors sequentially;
4. advance only after a provider timeout, rate limit, provider 5xx, network failure, empty dataset item, invalid provider response, or oversized provider response;
5. stop immediately on provider authentication/authorization errors, invalid requests, or unclassified failures rather than multiplying bad paid calls.

The checked-in default is two attempts: `apify/web-scraper` then `apify/playwright-scraper`. Already-allowlisted JS-heavy ATS hosts (`*.greenhouse.io`, `*.lever.co`, `*.myworkdayjobs.com`, `*.workday.com`) prefer Playwright first and Web Scraper second. Operators can supply at most three Actor IDs globally or per domain.

The maintained Actors do not share one identical execution model. `apify/web-scraper` runs its page function in the browser/DOM context; `apify/playwright-scraper` and `apify/puppeteer-scraper` execute server-side Node page functions around a browser `page`. The service therefore uses an explicit input adapter for the two known browser Actors. Unknown/private Actor IDs intentionally retain the existing Web-Scraper-compatible input contract; do not add an arbitrary Store Actor to a chain unless its input/output contract is compatible or an adapter is added and tested.

## Time and cost budgets

Every Actor attempt remains constrained to one start URL, an empty `linkSelector`, one page/result, and Actor concurrency 1. `respectRobotsTxtFile=true` is forced for every profile even when a local request-level override was authorized. Page retries default to 2 and pod-level external **chain** concurrency defaults to 1.

`SCRAPER_MAX_TOTAL_TIMEOUT_MS` bounds the local attempt + fallback delay + entire external chain. The chain partitions the remaining provider window across remaining Actors rather than giving every Actor the full timeout.

Apify's `maxTotalChargeUsd` is a per-run limit. `APIFY_MAX_TOTAL_CHARGE_USD` caps each Actor run, while `APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD` caps the sum of all possible Actor run ceilings for one local failure. The chain divides the aggregate ceiling across the configured attempts before issuing the first call. The default two-attempt chain is therefore bounded to USD 0.50 in aggregate and USD 0.25 per Actor run.

A provider-chain failure trips the existing cooldown. A successful Actor stops the chain immediately.

The Apify token is sent only as `Authorization: Bearer …`; it is never placed in the request URL or emitted by supervisor logs/status/metrics.

## flags-2-env

The service root contains `.cli-flags.toml`, the `flags-2-env` contract for non-secret runtime settings. The canonical package is `flags-2-env/flags-2-env` / `@oresoftware/f2e`. The production entrypoint executes `flags2env` before telemetry, listeners, browser processes, or the scraper core and materializes its resolved/default values into `process.env`; these defaults are executable runtime policy rather than documentation.

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
| `APIFY_FALLBACK_ENABLED` | `false` in flags contract; deployment opts in | master external-fallback switch |
| `APIFY_TOKEN` | unset | secret API token; required for any external call |
| `APIFY_ACTOR_ID` | `apify/web-scraper` | compatibility/default Actor when no chain is supplied |
| `APIFY_FALLBACK_ACTORS` | `apify/web-scraper,apify/playwright-scraper` | ordered default Actor chain, max 3 |
| `APIFY_FALLBACK_MAX_ATTEMPTS` | `2` | max Actor attempts after one eligible local failure, hard max 3 |
| `APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD` | `0.50` | aggregate worst-case Actor run-cap ceiling |
| `APIFY_DOMAIN_FALLBACKS_JSON` | ATS route map | exact/`*.suffix` domain to ordered Actor arrays |
| `APIFY_API_BASE_URL` | `https://api.apify.com/v2` | HTTPS API base URL |
| `APIFY_FALLBACK_TIMEOUT_MS` | `90000` | upper bound for one Actor; chain partitions remaining time |
| `APIFY_MAX_REQUEST_RETRIES` | `2` | page retries inside each Actor |
| `APIFY_FALLBACK_MAX_CONCURRENT` | `1` | pod-level external chain concurrency cap |
| `APIFY_FALLBACK_MIN_DELAY_MS` | `1000` | minimum delay before remote chain |
| `APIFY_FALLBACK_FAILURE_COOLDOWN_MS` | `60000` | cooldown after an exhausted provider chain |
| `APIFY_MAX_TOTAL_CHARGE_USD` | `0.25` | per-Actor-run billing ceiling |
| `APIFY_FALLBACK_FOR_EXPLICIT_STRATEGY` | `false` | permit fallback after explicit local strategies |
| `APIFY_MAX_RESPONSE_BYTES` | `2000000` | response buffer cap per Actor attempt |

Example operator override:

```bash
flags2env __dd_web_scraper__ \
  --apify-fallback-actors=apify/web-scraper,apify/playwright-scraper,apify/puppeteer-scraper \
  --apify-fallback-max-attempts=3 \
  --apify-fallback-chain-max-total-charge-usd=0.60 \
  --apify-domain-fallbacks-json='{"*.example.com":["apify/playwright-scraper","apify/web-scraper"]}'
```

## TJSV contract boundary

`contracts/scraper-fallback.tsp` and `contracts/scraper-fallback.schema.json` remain independently authored peer authorities. TJSV is the fail-closed admission mechanism. The fallback provenance contract includes bounded Actor-attempt evidence (`actorId`, outcome, duration, optional error class), selected route, maximum chain attempts, and the aggregate charge ceiling. Generated schema/output is evidence only.

## Observability

`GET /status` adds a non-secret `externalFallback` descriptor. `GET /fallback/status` returns provider/chain configuration without the token or configured domain patterns. Existing `/metrics` output is augmented with request-level chain attempts/success/failure/in-flight counts, skipped failures by classifier reason, and per-Actor success/error counters.

A successful external result uses `strategy: "apify"`, `provider: "apify"`, and a `fallback` object describing the local status/strategy, selected route, Actor attempts, and timing. Opt-in lead/contact extraction reuses the existing `contacts.ts` normalization logic.
