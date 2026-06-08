# dd-data-viz-rs audit

Last updated: 2026-06-08

This audit tracks the current hardening and visualization-platform parity posture for
`dd-data-viz-rs`.

## Current proof points

- The service is no longer only a monolithic `main.rs`; platform parity lives in
  `src/platform.rs`, hardening posture lives in `src/hardening.rs`, RBAC policy lives in
  `src/rbac.rs`, saved dashboard validation lives in `src/dashboard.rs`, parser-backed SQL
  compilation lives in `src/sql_frontend.rs`, shared helpers live in `src/util.rs`, and the HTTP
  server wires those modules through route handlers.
- Operator data-bearing endpoints are protected by `SERVER_AUTH_SECRET` unless
  `DATA_VIZ_ALLOW_UNAUTHENTICATED=true` is explicitly enabled for local development.
- Protected endpoints enforce `data-viz.rbac.v1` roles through `X-Data-Viz-Role` or `X-DD-Role`,
  defaulting a valid role-less operator request to `admin` for compatibility.
- Dataset ingestion is bounded by HTTP body bytes, dataset count, row count, and column count.
- Query responses are bounded by `MAX_QUERY_ROWS`.
- Saved dashboard definitions are validated, bounded, and exposed through `POST /dashboards`,
  `GET /dashboards`, and `GET /dashboards/:dashboard_id`.
- SQL requests are parsed through `sqlparser` and fail closed on joins, CTEs, set operations,
  unsupported predicates, and unsupported aggregate shapes.
- Categorical columns are dictionary encoded and exposed through profiles.
- Qlik-style associative exploration has a concrete first slice through
  `GET /associations/:dataset_id`, which emits co-occurrence support and confidence edges across
  categorical fields.
- Platform parity is visible at `GET /capabilities/parity`, covering Tableau, Power BI, Qlik,
  Looker, Sigma, Domo, Superset, Metabase, Grafana, D3.js, Plotly/Dash, and Evidence.dev.
- Hardening posture is visible at `GET /security/policy`, including implemented controls and
  residual risks.

## Remaining parity gaps

- Tableau parity still needs persisted dashboard layouts, renderer screenshots, interaction tests,
  and workbook publishing.
- Power BI parity still needs DAX and Power Query M parsers plus incremental refresh partitions.
- Qlik parity still needs a multi-dataset associative index and selection-state engine.
- Looker parity still needs LookML parsing, semantic-model validation, and SQL compilation targets.
- Sigma parity still needs a virtual spreadsheet engine with lazy paging over warehouse-backed
  result sets.
- Domo parity still needs a connector SDK, streaming checkpoints, and a visual ETL job runner.
- Superset and Metabase parity still need saved charts/questions, RBAC-backed ownership, and
  database connection registries beyond the current in-memory dashboard catalog and role policy.
- Grafana parity still needs alert rules, Loki log frames, and live WebSocket panel streams.
- D3, Plotly/Dash, and Evidence parity still need generated client packages and rendered artifact
  verification.

## Hardening gaps

- The current auth model has RBAC permissions, but identity is still derived from role headers plus
  shared-secret operator auth rather than gateway-backed user identity and per-resource policies.
- Non-SQL dialect parsing is subset-oriented. Full compatibility requires GraphQL, PromQL/LogQL,
  Flux, Cypher, Gremlin, Mongo, JMESPath, Lucene, and SPL parser crates or stricter AST validation.
- Datasets and evolution runs are in-memory only; durable Arrow/Parquet spill and TTL cleanup are
  required before long-running multi-tenant use.
- Presentation export returns inert package layers and API blueprints; final `.pptx` generation and
  Google Slides API calls belong in a separately authenticated artifact worker.
