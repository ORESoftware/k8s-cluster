# dd-economics-server

Rust economics dashboard and forecast service for the remote runtime.

The service blends observed market history with transparent economics priors. It is not an
exchange executor and does not place trades. It serves a dashboard, accepts normalized market
series, can pull JSON from approved public/private API URLs, tracks fiscal/labor and VC-flow
context, ranks invest/dump candidates, and projects each instrument for the next 18 months with
confidence intervals.

## Endpoints

- `GET /` - HTML dashboard shell.
- `GET /descriptor` - service descriptor with defaults, NATS subjects, and DES engine surface.
- `GET /dashboard.json` - authenticated dashboard data and projections.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe.
- `GET /metrics` - Prometheus text metrics.
- `GET /schema` - request/response contract summary for `economics.forecast.v1`.
- `GET /example` - sample forecast request.
- `GET /sources` - public/private source catalog.
- `POST /sources/pull` - authenticated JSON API pull and optional parse into a market series.
- `GET /sentiment/sources` - social/news sentiment provider catalog and configured credential flags.
- `POST /sentiment/analyze` - authenticated placeholder sentiment analysis over supplied social/news snippets.
- `GET /macro/indicators` - fiscal, borrowing, spending, GDP, labor participation, payroll, wage, and productivity context.
- `GET /vc/investment` - VC firm/deal/sector-flow sample context and private-market credential flags.
- `POST /recommendations` - authenticated top 20 company invest/dump and top 30 commodity buy/sell-or-dump rankings.
- `POST /ingest` - authenticated normalized market series ingestion.
- `GET /model/equations` - equation and theory catalog.
- `GET /engine/des` - `remote/submodules/discrete-event-system.rs` SDK/service descriptor.
- `POST /forecast` - authenticated forecast endpoint.

`POST /forecast`, `POST /ingest`, `POST /sources/pull`, `POST /sentiment/analyze`,
`POST /recommendations`, and `GET /dashboard.json` require `X-Server-Auth` or `Auth` to match
`SERVER_AUTH_SECRET`, unless `ECONOMICS_ALLOW_UNAUTHENTICATED=true` is set for local development.

## Model stance

The forecast engine is intentionally explainable. It combines:

- data drift from annualized log returns
- GBM-style confidence intervals
- Ornstein-Uhlenbeck mean reversion
- CAPM risk-asset priors
- Fisher/Taylor/Phillips/quantity-theory macro priors
- UIP/PPP FX priors
- Hotelling/carry commodity priors
- duration/rate sensitivity for bonds
- logistic/adoption and liquidity pressure for crypto and emerging markets
- fiscal drag from debt, borrowing, deficits, spending/receipts, and interest outlays
- labor-force participation, payroll, wage, and productivity impulses
- VC firm investment and private-market sector-flow read-throughs

Every projection returns a component ledger so the dashboard can show how data, macro theory,
momentum, carry, valuation, and jump-stress terms contributed.

## Runtime contract

The default Kubernetes deployment runs with:

- `ECONOMICS_HISTORY_YEARS=15`
- `ECONOMICS_PROJECTION_MONTHS=18`
- `ECONOMICS_CONFIDENCE_LEVEL=0.90`
- `ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=false`
- `ECONOMICS_FORECAST_REQUEST_SUBJECT=dd.remote.economics.forecast.requests`
- `ECONOMICS_FORECAST_RESULT_SUBJECT=dd.remote.economics.forecast.results`
- `ECONOMICS_MARKET_EVENT_SUBJECT=dd.remote.economics.market.events`
- `ECONOMICS_RUNTIME_EVENT_SUBJECT=dd.remote.events`

Optional sentiment-provider placeholders are mounted from `dd-agent-secrets` when present:

- X/Twitter: `ECONOMICS_X_BEARER_TOKEN`, `ECONOMICS_X_API_KEY`,
  `ECONOMICS_X_API_SECRET`, `ECONOMICS_X_ACCESS_TOKEN`,
  `ECONOMICS_X_ACCESS_TOKEN_SECRET`
- Reddit: `ECONOMICS_REDDIT_CLIENT_ID`, `ECONOMICS_REDDIT_CLIENT_SECRET`,
  `ECONOMICS_REDDIT_USER_AGENT`
- News/social/event feeds: `ECONOMICS_NEWS_API_KEY`, `ECONOMICS_STOCKTWITS_TOKEN`,
  `ECONOMICS_GDELT_API_KEY`

Optional macro, fiscal, labor, commodities, filings, and VC/private-market placeholders are also
mounted from `dd-agent-secrets` when present:

- Public macro/fiscal/labor: `ECONOMICS_FRED_API_KEY`, `ECONOMICS_BEA_API_KEY`,
  `ECONOMICS_BLS_API_KEY`, `ECONOMICS_TREASURY_API_KEY`, `ECONOMICS_CENSUS_API_KEY`
- Commodities and filings: `ECONOMICS_EIA_API_KEY`, `ECONOMICS_SEC_API_KEY`
- VC/private markets: `ECONOMICS_CRUNCHBASE_API_KEY`, `ECONOMICS_PITCHBOOK_API_KEY`,
  `ECONOMICS_CB_INSIGHTS_API_KEY`, `ECONOMICS_DEALROOM_API_KEY`, `ECONOMICS_PREQIN_API_KEY`

Private API tokens belong in Kubernetes/AWS secrets and are referenced by env var name in
`POST /sources/pull` or future live sentiment fetchers; token values are never returned by the
service. `POST /sentiment/analyze` is deliberately useful before live connectors exist: callers can
submit X, Reddit, news, RSS, or forum text documents and receive bounded placeholder sentiment
scores by source.

`POST /recommendations` is likewise useful before live connectors exist. It accepts optional
`macroFiscalContext`, `ventureCapitalContext`, `sentimentContext`, and market `series` overrides,
then returns model-signal rankings with component ledgers. Rankings are research signals, not
financial advice or trade execution instructions.
