# dd-economics-server

Rust economics dashboard and forecast service for the remote runtime.

The service blends observed market history with transparent economics priors. It is not an
exchange executor and does not place trades. It serves a dashboard, accepts normalized market
series, can pull JSON/CSV from approved public/private API URLs, tracks fiscal/labor and VC-flow
context, ranks invest/dump candidates, and projects each instrument for the next 18 months with
confidence intervals.

## Endpoints

- `GET /` - HTML dashboard shell.
- `GET /descriptor` - service descriptor with defaults, NATS subjects, and DES engine surface.
- `GET /dashboard.json` - authenticated dashboard data and projections.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe.
- `GET /metrics` - Prometheus text metrics.
- `GET /observability` - telemetry posture, Prometheus/Loki/Grafana hints, and runtime cardinality.
- `GET /schema` - request/response contract summary for `economics.forecast.v1`.
- `GET /example` - sample forecast request.
- `GET /sources` - public/private source catalog.
- `GET /sources/public` - known public sourceId templates with URLs, parsers, and source docs.
- `POST /sources/pull` - authenticated sourceId or custom API pull and optional parse into a market series.
- `GET /sentiment/sources` - social/news sentiment provider catalog and configured credential flags.
- `POST /sentiment/analyze` - authenticated placeholder sentiment analysis over supplied social/news snippets.
- `GET /macro/indicators` - fiscal, borrowing, spending, GDP, labor participation, payroll, wage, and productivity context.
- `GET /vc/investment` - VC firm/deal/sector-flow sample context and private-market credential flags.
- `POST /recommendations` - authenticated top 20 company invest/dump and top 30 commodity buy/sell-or-dump rankings.
- `GET /audit/hardening` - hardening posture, request bounds, egress gates, and residual risks.
- `GET /pipelines/catalog` - Spark, Airflow, Databricks, data lake, and NATS integration catalog.
- `POST /pipelines/plan` - authenticated redacted big-data job intents for economics refresh work.
- `POST /pipelines/submit` - authenticated Spark pipeline submission when explicitly enabled.
- `POST /ingest` - authenticated normalized market series ingestion.
- `GET /model/equations` - equation and theory catalog.
- `GET /engine/des` - `remote/submodules/discrete-event-system.rs` SDK/service descriptor.
- `POST /forecast` - authenticated forecast endpoint.

`POST /forecast`, `POST /ingest`, `POST /sources/pull`, `POST /sentiment/analyze`,
`POST /recommendations`, `POST /pipelines/plan`, `POST /pipelines/submit`, and
`GET /dashboard.json` require `X-Server-Auth`, `Auth`, or `Authorization: Bearer ...` to match
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
- `ECONOMICS_ALLOWED_SOURCE_HOSTS=api.fiscaldata.treasury.gov,api.worldbank.org,api.coingecko.com,fred.stlouisfed.org`
- `ECONOMICS_FORECAST_REQUEST_SUBJECT=dd.remote.economics.forecast.requests`
- `ECONOMICS_FORECAST_RESULT_SUBJECT=dd.remote.economics.forecast.results`
- `ECONOMICS_MARKET_EVENT_SUBJECT=dd.remote.economics.market.events`
- `ECONOMICS_RUNTIME_EVENT_SUBJECT=dd.remote.events`
- `ECONOMICS_PIPELINE_INTENT_SUBJECT=dd.remote.public_data.pipeline.jobs`
- `ECONOMICS_SPARK_PIPELINE_URL=http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085`
- `ECONOMICS_SPARK_PIPELINE_AUTH_ENV=SERVER_AUTH_SECRET`
- `ECONOMICS_SPARK_MASTER_URL=spark://spark-master.big-data.svc.cluster.local:7077`
- `ECONOMICS_AIRFLOW_API_URL=http://airflow.big-data.svc.cluster.local:8080`
- `ECONOMICS_DATA_LAKE_URI=s3a://dd-economics/market-signals`
- `ECONOMICS_ENABLE_PIPELINE_SUBMIT=false`
- `ECONOMICS_ALLOW_EXTERNAL_PIPELINE_URLS=false`
- `OTEL_SERVICE_NAME=dd-economics-server`
- `OTEL_SERVICE_NAMESPACE=remote-dev`
- `OTEL_RESOURCE_ATTRIBUTES=deployment.environment=stage,service.version=0.1.0,k8s.namespace.name=default`
- `ECONOMICS_GRAFANA_DASHBOARD_UID=dd-economics-server`

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
- Commodities, crypto, and filings: `ECONOMICS_EIA_API_KEY`,
  `ECONOMICS_COINGECKO_API_KEY`, `ECONOMICS_SEC_API_KEY`
- VC/private markets: `ECONOMICS_CRUNCHBASE_API_KEY`, `ECONOMICS_PITCHBOOK_API_KEY`,
  `ECONOMICS_CB_INSIGHTS_API_KEY`, `ECONOMICS_DEALROOM_API_KEY`, `ECONOMICS_PREQIN_API_KEY`
- Databricks managed workspace placeholders: `ECONOMICS_DATABRICKS_HOST`,
  `ECONOMICS_DATABRICKS_TOKEN`

Private API tokens belong in Kubernetes/AWS secrets and are referenced by env var name in
`POST /sources/pull` or future live sentiment fetchers; token values are never returned by the
service. `POST /sentiment/analyze` is deliberately useful before live connectors exist: callers can
submit X, Reddit, news, RSS, or forum text documents and receive bounded placeholder sentiment
scores by source.

`POST /recommendations` is likewise useful before live connectors exist. It accepts optional
`macroFiscalContext`, `ventureCapitalContext`, `sentimentContext`, and market `series` overrides,
then returns model-signal rankings with component ledgers. Rankings are research signals, not
financial advice or trade execution instructions.

## Observability

`GET /metrics` exposes Prometheus counters for HTTP requests, authenticated forecast/ingest/pipeline
work, source pull success/failure, source pull response bytes, stored source points, and the last
successful source pull unix timestamp. `GET /observability` returns the same telemetry contract as
structured JSON for dashboards and operators.

The service writes compact `dd.log.v1` JSON log envelopes to stdout/stderr so Promtail/Loki can
collect startup, auth failure, NATS, pipeline, and source-pull events from container logs. OpenTelemetry
metadata is explicit-only: use the `OTEL_*` env vars for service identity and resource attributes,
but do not add runtime monkey-patching or auto-instrumentation. Grafana dashboards should start with
the UID in `ECONOMICS_GRAFANA_DASHBOARD_UID` and chart request totals, source pull health, forecast
latency/error logs, and `/readyz` availability.

## Public source templates

`GET /sources/public` returns sourceId templates for official/public data that can be pulled with:

```json
{
  "sourceId": "treasury-debt-to-penny"
}
```

Current templates cover Treasury public debt, World Bank US GDP and labor participation,
CoinGecko BTC/ETH public market charts, and FRED CSV feeds for 10-year Treasury rates, WTI oil,
gold, silver, S&P 500, 30-year mortgage rates, and USD/EUR FX. `POST /sources/pull` resolves a
sourceId to immutable URL/parser metadata and stores a normalized `MarketSeries` with a quality
report containing observed points, dropped points, first/last date, and min/max price.

Custom pulls remain available for private APIs, but are gated: source URLs cannot contain
credentials or fragments, redirects are not followed, private/link-local hosts and custom ports are
blocked unless `ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true`, and
`ECONOMICS_ALLOWED_SOURCE_HOSTS` can restrict ad-hoc public egress. Known parser modes are
`json-records`, `json-tuple-array`, and `csv-records`.

CoinGecko's unauthenticated public API currently limits historical market chart pulls to the past
365 days; use `ECONOMICS_COINGECKO_API_KEY` or a private exchange/vendor feed for longer crypto
history.

## Big-data pipeline integration

`POST /pipelines/plan` turns the current economics universe into redacted job intents for:

- `dd-spark-pipeline-server` `INGEST_VALIDATE_PUBLISH` and `SPARK_SUBMIT` jobs
- the development Spark master in `big-data`
- an Airflow `economics_market_refresh` DAG trigger blueprint
- a Databricks Jobs API `run-now` blueprint
- the generated NATS subject `dd.remote.public_data.pipeline.jobs`
- public source IDs from `GET /sources/public` for upstream dataset refreshes

`POST /pipelines/submit` submits only the Spark pipeline server intents, only to a cluster-local
HTTP URL by default, and only when `ECONOMICS_ENABLE_PIPELINE_SUBMIT=true`. Airflow and Databricks
remain plan-only until their service auth and audit flows are explicitly designed.

## Local checks

```sh
cargo fmt
cargo test
curl -fsS http://127.0.0.1:8114/observability
cargo test public_source_templates_fetch_live_external_data_when_available -- --ignored --nocapture
```

The ignored test uses live public Treasury, World Bank, and CoinGecko endpoints. It is intentionally
manual so CI does not fail when an external provider is down, rate-limited, or changes its public
access policy.
