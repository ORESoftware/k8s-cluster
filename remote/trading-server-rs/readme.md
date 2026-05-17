# dd-trading-server

Rust algorithmic trading decision service for the remote runtime.

This service is intentionally an orchestrator, not an exchange executor. It accepts market,
scraper, AI/ML feature, and MDP/POMDP policy signals, produces a bounded buy/sell/hold decision,
and publishes decision/order-intent events to NATS. It does not hold broker credentials or submit
orders to an exchange.

## Endpoints

- `GET /` - service descriptor with upstream service URLs, NATS subjects, and safety mode.
- `GET /healthz` - health probe.
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
`remote/databases/pg/seeds/trading-platform-app-config.sql`. The seeded config includes:

- `interactive-brokers`
- `alpaca`
- `tradier`
- `coinbase-advanced-trade`
- `kraken`
- `gemini`
- `binance-us`
- `polymarket` (paused by default)
- `factmachine` (paused placeholder)

The app-config row stores endpoint/profile metadata and Kubernetes secret key names only. Raw API
tokens, account IDs, and gateway URLs belong in `dd-trading-broker-secrets` or the existing AWS
Secrets Manager path that syncs into Kubernetes.

The service can subscribe to `dd.remote.trading.signals` and publish decisions/intents without
going through HTTP. A future executor should consume order intents and enforce its own risk checks
before placing paper or live orders.

## Safety stance

The decision engine uses simple bounded scoring today:

- web sentiment/relevance/confidence
- AI/ML feature values
- recent market momentum
- MDP/POMDP policy hints

Safety gates can force `hold` even when the model recommends `buy` or `sell`: missing/paused
platform config, unsupported paper/live mode, disabled mode, live gate off, low confidence, high
risk score, missing price, shorting disallowed, or exposure limits. Live mode still only emits an
intent; it never calls an exchange API.
