# dd-trading-server

Rust algorithmic trading decision service for the remote runtime.

This service is intentionally an orchestrator, not an exchange executor. It accepts market,
scraper, AI/ML feature, and MDP/POMDP policy signals, produces a bounded buy/sell/hold decision,
and publishes decision/order-intent events to NATS. It does not hold broker credentials or submit
orders to an exchange.

## Endpoints

- `GET /` - service descriptor with upstream service URLs, NATS subjects, and safety mode.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe that fails when platform config is missing or stale.
- `GET /metrics` - Prometheus text metrics.
- `GET /schema` - request/response contract summary for `trading.decision.v1`.
- `GET /example` - sample decision request.
- `POST /decide` - authenticated decision endpoint.

`POST /decide` requires `X-Server-Auth` or `Auth` to match `SERVER_AUTH_SECRET`, unless
`TRADING_ALLOW_UNAUTHENTICATED=true` is set for local development.

## Runtime contract

The default Kubernetes deployment runs with:

- `TRADING_MODE=paper`
- `TRADING_ALLOW_LIVE_ORDERS=false`
- `TRADING_APP_CONFIG_SCOPE=default`
- `TRADING_APP_CONFIG_KEY=trading.platforms.v1`
- `TRADING_CONFIG_REFRESH_SECONDS=30`
- `TRADING_SIGNAL_SUBJECT=dd.remote.trading.signals`
- `TRADING_DECISION_SUBJECT=dd.remote.trading.decisions`
- `TRADING_ORDER_INTENT_SUBJECT=dd.remote.trading.order_intents`
- `TRADING_EVENT_SUBJECT=dd.remote.events`
- upstream descriptors for `dd-web-scraper`, `dd-ai-ml-pipeline`, and `dd-mdp-optimizer`

Platform metadata is loaded from the generic RDS `app_config` table using
`remote/databases/pg/seeds/trading-platform-app-config.sql`. The seeded config covers 15+ active
platforms, weighted toward well-understood commodities/futures:

Brokerages & multi-asset:

- `interactive-brokers`
- `alpaca`
- `tradier`
- `tradestation` (equities, options, futures, commodities)
- `saxo` (multi-asset incl. commodity futures/CFDs)

Commodities / futures specialists:

- `tradovate` (futures: energy, metals, ags, index)
- `ironbeam` (commodity futures)
- `oanda` (forex + spot metals + commodity CFDs)
- `ig` (commodity/index/FX CFDs)
- `cqg` (futures/commodities routing via in-cluster WebAPI gateway)
- `amp-futures` (futures via CQG/Rithmic gateway)

Crypto:

- `coinbase-advanced-trade`
- `kraken`
- `gemini`
- `binance-us`

Paused by default:

- `polymarket` (paused by default)
- `factmachine` (paused placeholder)

For `cqg` and `amp-futures` the seeded `baseUrls` point at an in-cluster loopback gateway
(mirroring the existing `interactive-brokers` pattern), not a public host: those networks are
FIX/proprietary and a future executor must terminate the gateway and supply auth.

The app-config row stores endpoint/profile metadata and Kubernetes secret key names only. Raw API
tokens, account IDs, and gateway URLs belong in `dd-trading-broker-secrets` or the existing AWS
Secrets Manager path that syncs into Kubernetes; this decision pod does not mount those broker
secrets.

The service can subscribe to `dd.remote.trading.signals` and publish decisions/intents without
going through HTTP. A future executor should consume order intents and enforce its own risk checks
before placing paper or live orders.

## Safety stance

The decision engine uses simple bounded scoring today:

- web sentiment/relevance/confidence
- AI/ML feature values
- recent market momentum
- MDP/POMDP policy hints

Per-request `constraints` can only tighten the server defaults. Safety gates can force `hold` even
when the model recommends `buy` or `sell`: kill switch engaged, missing/paused
platform config, unsupported paper/live mode, disabled mode, live gate off, low confidence, high
risk score, missing price, shorting disallowed, or exposure limits. Live mode still only emits an
intent; it never calls an exchange API.

## Operational hardening

- **Kill switch** — `TRADING_HALT=true` forces every decision to `hold` via the `tradingNotHalted`
  safety gate, without restarting or descheduling the pod. Use it as a fast circuit breaker.
- **Concurrency cap** — `TRADING_MAX_INFLIGHT` (default 256) bounds how many NATS-sourced
  decisions evaluate at once; the subscription applies backpressure instead of spawning unbounded
  tasks under a signal flood.
- **NATS auth/TLS** — the broker connection sets a stable client name, pings, and a connect timeout,
  retries the initial connect, and supports optional auth via `NATS_CREDENTIALS_FILE`,
  `NATS_TOKEN`, or `NATS_NKEY`, plus `NATS_REQUIRE_TLS=true`.
- **Readiness** — `/readyz` reports ready whenever a usable platform catalog is loaded. A transient
  RDS/CDC refresh error is surfaced as `lastConfigError` and counted in
  `dd_trading_server_config_refresh_failures_total` rather than pulling every replica out of
  rotation on a shared-DB blip (the last-good config keeps serving).
- **Loopback URL validation** — platform `baseUrls` must be `https://` or a true loopback host
  (`localhost`/`127.0.0.1`/`[::1]` followed by `:`, `/`, or end), so look-alikes like
  `http://localhost.evil.com` are rejected.

## Executor boundary (read before wiring live trades)

"Bots finding and executing trades" is intentionally split across **two** services. This pod is the
**decision** half: it scores signals and emits intent-only `trading.order_intent` events; it holds
no broker credentials and never contacts an exchange. Actually **placing** orders is a separate,
not-yet-built **executor** that must own broker secrets and add its own controls before any live
fill: per-venue idempotency/dedup of intents (NATS can redeliver), live notional/rate budgets,
reconciliation against fills, and per-platform eligibility (e.g. geo, market rules). Until that
executor exists and is signed off, keep `TRADING_MODE=paper` and `TRADING_ALLOW_LIVE_ORDERS=false`.
