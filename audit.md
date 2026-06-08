# dd-data-viz-rs audit

Last updated: 2026-06-08

This audit tracks the current hardening and visualization-platform parity posture for
`dd-data-viz-rs`.

## Current proof points

- The service is no longer only a monolithic `main.rs`; platform parity lives in
  `src/platform.rs`, Grafana-style alert rules live in `src/alerts.rs`, alert notification policy
  validation lives in `src/notifications.rs`, alert notification dispatch outbox records live in
  `src/notification_dispatch.rs`, Loki log frame conversion lives in `src/loki_frames.rs`, live
  panel WebSocket snapshots live in `src/live_panels.rs`, Qlik-style selection state lives in
  `src/associative.rs`, semantic model parsing and compilation lives in `src/semantic.rs`, ETL flow
  planning lives in `src/etl.rs`, secretRef-backed data connection metadata lives in
  `src/connections.rs`, infrastructure diagram extraction lives in `src/infra_diagrams.rs`,
  Evidence-style report compilation lives in `src/evidence_reports.rs`, renderer client package
  blueprints live in `src/renderer_packages.rs`, hardening posture lives in `src/hardening.rs`, RBAC
  policy lives in `src/rbac.rs`, saved dashboard validation lives in `src/dashboard.rs`, publishing
  approval validation lives in `src/publishing.rs`, Power BI-style DAX expression parsing lives in
  `src/dax.rs`, self-service question/chart validation lives in `src/self_service.rs`,
  natural-language question planning lives in `src/question_nl.rs`, SQL Lab history validation lives
  in `src/sql_lab.rs`, query result cache bounds live in `src/query_cache.rs`, Sigma-style workbook
  grid paging lives in `src/workbook_grid.rs`, parser-backed SQL compilation lives in
  `src/sql_frontend.rs`, shared helpers live in `src/util.rs`, and the HTTP server wires those
  modules through route handlers.
- Operator data-bearing endpoints are protected by `SERVER_AUTH_SECRET` unless
  `DATA_VIZ_ALLOW_UNAUTHENTICATED=true` is explicitly enabled for local development.
- Protected endpoints enforce `data-viz.rbac.v1` roles through `X-Data-Viz-Role` or `X-DD-Role`,
  defaulting a valid role-less operator request to `admin` for compatibility.
- Dataset ingestion is bounded by HTTP body bytes, dataset count, row count, and column count.
- Query responses are bounded by `MAX_QUERY_ROWS`.
- Saved dashboard definitions are validated, bounded, and exposed through `POST /dashboards`,
  `GET /dashboards`, and `GET /dashboards/:dashboard_id`.
- Superset/Metabase-style saved questions are validated against ingested dataset fields and exposed
  through `POST /questions`, `GET /questions`, `GET /questions/:question_id`, and `GET /charts`
  with role-gated read/write permissions.
- Metabase-style natural-language question proposals are generated through `POST /questions/nl` and
  `GET /questions/suggestions/:dataset_id` using deterministic field-name matching over dataset
  metadata, without model calls or query execution.
- Tableau/Superset-style publishing approval requests validate saved dashboard, question, or chart
  targets, keep bounded pending/approved/rejected state, and expose role-gated create/list/detail
  and review endpoints through `/publishing/requests`.
- Superset-style SQL Lab history is validated through `POST /sql-lab/history`, exposed through
  `GET /sql-lab/history` and `GET /sql-lab/history/:history_id`, rejects mutating or secret-looking
  SQL, and stores external connection entries as dry-run plans only.
- SQL requests are parsed through `sqlparser` and fail closed on joins, CTEs, set operations,
  unsupported predicates, and unsupported aggregate shapes.
- Successful `POST /query` responses write bounded in-memory result snapshots with `cacheId`, while
  `GET /query-cache` returns summaries without raw query text and `GET /query-cache/:cache_id`
  returns role-gated cached rows until TTL expiry.
- Categorical columns are dictionary encoded and exposed through profiles.
- Qlik-style associative exploration has a concrete first slice through
  `GET /associations/:dataset_id`, which emits co-occurrence support and confidence edges across
  categorical fields.
- Qlik-style multi-dataset selection is exposed through `POST /associations/select`, including
  selected, possible, alternative, and excluded categorical values propagated by shared field/value
  relationships.
- Qlik-style saved selection sessions are exposed through `POST /associations/sessions`,
  `GET /associations/sessions`, and `GET /associations/sessions/:session_id`, with bounded
  metadata and detail reads that recompute current green/white/gray state against loaded datasets.
- Qlik-style relationship discovery is exposed through `POST /associations/relationships`, scoring
  likely cross-dataset categorical relationships with value-overlap, coverage, field-name
  similarity, alias-kind labels, and confidence strengths.
- Grafana-style alert rules are validated, stored in memory, and evaluated through
  `POST /alerts/rules/:rule_id/evaluate` using reducer/threshold conditions over existing query
  results.
- Grafana-style alert contact points and notification policies are validated through
  `POST /alerts/contact-points`, `POST /alerts/notification-policies`, and
  `POST /alerts/rules/:rule_id/notification-preview` without storing raw tokens or sending
  outbound messages.
- Loki-native query results and structured log streams can be adapted through
  `POST /observability/loki/frame` into bounded, redacted Grafana-style log frame rows with label
  keys and level counts.
- Grafana-style live panel streams are exposed through `GET /live/panels/:dataset_id`, using a
  bounded WebSocket snapshot stream over ingested datasets with capped ticks, interval, fields, and
  returned rows.
- Grafana-style alert dispatch is exposed through `POST /alerts/rules/:rule_id/dispatch`, with
  bounded in-memory dispatch records available at `GET /alerts/dispatches` and
  `GET /alerts/dispatches/:dispatch_id`; delivery attempts reference secretRef handoff plans rather
  than raw credentials.
- Looker-style semantic models are parsed from a bounded LookML-like subset, validated against
  ingested dataset fields, stored in memory, and compiled into SQL plus `LogicalPlan` through
  `POST /semantic/registry/:model_id/compile`.
- Power BI-style DAX expressions are parsed through a bounded subset compiler at
  `POST /expressions/dax/compile`, validated against ingested dataset fields, and returned as AST,
  dependencies, logical hints, and SQL preview without evaluating user formulas.
- Domo Magic ETL/Power Query-style flows are validated through `POST /etl/plans` against ingested
  dataset schemas, producing lineage, materialization, and connector pushdown hints without
  executing user formulas.
- Sigma-style virtual workbook grid pages are exposed through `POST /workbooks/grid/page`, returning
  bounded row windows with projection, filters, sorts, paging metadata, and validated formula-column
  plans over the in-memory columnar dataset.
- SecretRef-backed data connection metadata is validated through `POST /connections` and exposed
  through `GET /connections`, `GET /connections/:connection_id`, and
  `POST /connections/:connection_id/test-plan`; dry-run test plans do not open sockets or call cloud
  APIs.
- Terraform/HCL, Terraform plan JSON, AWS inventory, AWS Resource Explorer, GCP inventory, and GCP
  Cloud Asset inputs can generate neutral infrastructure graphs and diagram-as-code, whiteboard,
  interactive web graph, graph analytics, spatial, presentation, and Kroki-ready renderer targets
  through `GET /diagrams/tools` and `POST /diagrams/infra`.
- Evidence-style Markdown reports compile through `POST /reports/evidence/compile`, validating
  embedded SQL against ingested datasets before returning Markdown, chart placeholders, and dataset
  dependency metadata.
- Generated TypeScript renderer client package blueprints are exposed through
  `GET /renderers/client-package`, including D3 final-layer helpers, Plotly/Dash figure helpers,
  Evidence report helpers, infrastructure graph helpers, and deterministic file checksums.
- Platform parity is visible at `GET /capabilities/parity`, covering Tableau, Power BI, Qlik,
  Looker, Sigma, Domo, Superset, Metabase, Grafana, D3.js, Plotly/Dash, and Evidence.dev.
- Hardening posture is visible at `GET /security/policy`, including implemented controls and
  residual risks.

## Remaining parity gaps

- Tableau parity still needs persisted dashboard layouts, renderer screenshots, interaction tests,
  and durable workbook publication state beyond in-memory approvals.
- Power BI parity still needs broader DAX time-intelligence and row-context semantics, a Power Query
  M import parser, and incremental refresh partitions.
- Qlik parity still needs durable selection sessions beyond memory, operator-reviewed relationship
  override maps, and semantic relationship inference beyond categorical value overlap.
- Looker parity still needs full LookML project parsing, multi-view joins, Git-backed validation
  workflows, and live warehouse-specific SQL execution targets.
- Sigma parity still needs warehouse-backed lazy paging/execution and collaborative workbook state.
- Domo parity still needs a connector SDK, streaming checkpoints, durable ETL job execution, and a
  drag-and-drop visual flow builder.
- Superset and Metabase parity still need richer natural-language intent parsing, actual external
  connector execution, durable cache/storage backends, and durable ownership beyond the current
  in-memory role-gated connection/SQL-Lab/cache/question/chart/publishing catalog.
- Grafana parity still needs external notification channel drivers and durable dispatch state beyond
  the current bounded in-memory outbox.
- D3 and Plotly/Dash parity still need rendered artifact verification; Evidence parity still needs
  repo build artifacts and scheduled publication; infrastructure diagram parity still needs live
  cloud inventory connectors and richer cloud-native relationship extraction beyond topology hints.

## Hardening gaps

- The current auth model has RBAC permissions, but identity is still derived from role headers plus
  shared-secret operator auth rather than gateway-backed user identity and per-resource policies.
- Non-SQL dialect parsing is subset-oriented. Full compatibility requires GraphQL, PromQL/LogQL,
  Flux, Cypher, Gremlin, Mongo, JMESPath, Lucene, and SPL parser crates or stricter AST validation.
- Datasets and evolution runs are in-memory only; durable Arrow/Parquet spill and TTL cleanup are
  required before long-running multi-tenant use.
- Presentation export returns inert package layers and API blueprints; final `.pptx` generation and
  Google Slides API calls belong in a separately authenticated artifact worker.
