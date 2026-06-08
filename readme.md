# dd-data-viz-rs

Rust columnar analytics and evolutionary data visualization server.

This service is the first deployable slice of a mini-Tableau-style engine for the remote runtime.
It ingests JSON records into a cache-friendly in-memory column store, translates multiple query
dialects into one logical plan, executes grouped aggregations, synthesizes visualization specs for
2D/3D/4D/5D/XD data, mutates those specs with an evolutionary search loop, and emits presentation
layers for PowerPoint/OpenXML, Google Slides, Reveal markdown, and downstream renderers.

## Current engine shape

- Numeric columns are stored as contiguous `Vec<Option<f64>>`.
- Categorical columns are dictionary encoded as unique strings plus `u32` codes.
- Boolean columns are stored as `Vec<Option<bool>>`.
- Queries compile into a unified `LogicalPlan` with projection, filter, group-by, aggregation, and
  limit nodes.
- SQL queries compile through `sqlparser` before they become a `LogicalPlan`; unsupported SQL
  constructs fail closed instead of being string-sliced.
- The execution path is intentionally dependency-light for the first slice. The public API leaves
  room to swap the physical backend to Apache Arrow/DataFusion, SIMD kernels, and parallel chunk
  reducers later.

## Dialects

`POST /query` accepts these dialect frontends and maps them into the same internal plan:

- SQL
- GraphQL
- PromQL
- Flux
- InfluxQL
- LogQL
- Cypher
- Gremlin
- Mongo aggregation pipeline
- JMESPath
- Lucene-style search pipeline
- Splunk SPL-style `stats` pipeline

SQL is parser-backed through `sqlparser` for one-table `SELECT` analytics. The other frontends
currently implement useful analytics subsets: source selection, simple filters, grouping, and
`count`/`sum`/`avg`/`min`/`max` aggregations where the dialect naturally supports them.

## Platform parity map

The codebase now has dedicated platform and hardening modules instead of putting every product
concept in `main.rs`:

- `src/platform.rs` defines parity surfaces for Tableau, Power BI, Qlik Sense, Looker, Sigma,
  Domo, Superset, Metabase, Grafana, D3.js, Plotly/Dash, and Evidence.dev.
- `src/alerts.rs` owns Grafana-style alert rule validation, catalog metadata, and reducer-based
  evaluation responses.
- `src/notifications.rs` owns Grafana-style contact point validation, notification policies, and
  dry-run delivery previews.
- `src/associative.rs` owns Qlik-style multi-dataset selection state and relationship indexing.
- `src/semantic.rs` owns LookML-like semantic model parsing, dataset-field validation, and SQL
  target compilation.
- `src/etl.rs` owns Domo Magic ETL/Power Query-style metadata flow validation, lineage, and
  connector pushdown hints.
- `src/connections.rs` owns secretRef-backed data connection metadata and dry-run connection test
  plans for warehouse and BI planner surfaces.
- `src/infra_diagrams.rs` owns Terraform/HCL, Terraform plan JSON, AWS inventory, AWS Resource
  Explorer, GCP inventory, and GCP Cloud Asset graph extraction plus Mermaid, Graphviz, PlantUML,
  D2, Structurizr, Cytoscape, Draw.io, Excalidraw, Vega force, NetworkX, GEXF, Markmap, and
  Markdown inventory outputs.
- `src/hardening.rs` defines operator auth posture, input limits, implemented controls, and
  residual risks.
- `src/rbac.rs` defines enforced roles and permissions for protected endpoints.
- `src/dashboard.rs` owns saved dashboard request validation and in-memory dashboard metadata.
- `src/self_service.rs` owns Metabase/Superset-style saved question validation, chart bindings, and
  compiled SQL request metadata.
- `src/question_nl.rs` owns deterministic natural-language question suggestions and prompt-to-query
  proposal planning over dataset field metadata.
- `src/sql_lab.rs` owns Superset-style bounded SQL Lab history validation, summaries, and dry-run
  external connection records.
- `src/query_cache.rs` owns bounded in-memory query result snapshots, TTL pruning, and cache
  summaries that omit raw query text.
- `src/sql_frontend.rs` owns parser-backed SQL-to-`LogicalPlan` compilation.
- `src/util.rs` owns shared identifier, escaping, timestamp, header, and scalar-label helpers.

Current first-class parity surfaces:

- Power BI / Looker: governed semantic model descriptors, LookML-like registry ingestion,
  dataset-backed validation, SQL compile targets, warehouse connection metadata, DAX analogs, and
  calculated fields.
- Qlik Sense: `GET /associations/:dataset_id` builds a categorical co-occurrence graph over an
  ingested dataset, while `POST /associations/select` computes multi-dataset green/white/gray
  selection state.
- Sigma: workbook blueprints for live-grid and executive-card workflows plus warehouse connection
  metadata for live-query planning.
- Domo / Power Query: connector catalog, ETL planner primitives, and `POST /etl/plans` validation
  for bounded visual flows.
- Superset / Metabase: bounded SQL Lab history, natural-language question proposals, visual
  query-builder/self-service contracts, RBAC policy, secretRef-backed database connection registry,
  query result cache, saved dashboard catalog, saved questions, and saved chart bindings.
- Grafana: time-series dashboard panel catalog, PromQL/LogQL query frontends, metrics route, alert
  rule evaluation, contact points, and notification policy previews.
- D3.js / Plotly / Dash / Evidence.dev: renderer contracts, final-layer JSON, Plotly trace
  blueprint posture, infrastructure diagrams, and Markdown-plus-SQL report blueprint.

## Endpoints

- `GET /` - HTML operator home.
- `GET /descriptor` - service descriptor, storage model, dialect catalog, and route map.
- `GET /dialects` - query dialect catalog.
- `GET /capabilities/parity` - BI and visualization tool parity matrix.
- `GET /connectors/catalog` - connector catalog and ETL planner primitives.
- `POST /connections` - authenticated secretRef-backed data connection create/replace.
- `GET /connections` - authenticated data connection catalog.
- `GET /connections/:connection_id` - authenticated data connection definition.
- `POST /connections/:connection_id/test-plan` - authenticated dry-run connection test plan.
- `GET /semantic/models` - governed semantic models, dimensions, measures, and calculations.
- `POST /semantic/registry` - authenticated LookML-like semantic model create/replace.
- `GET /semantic/registry` - authenticated semantic model registry.
- `GET /semantic/registry/:model_id` - authenticated semantic model definition.
- `POST /semantic/registry/:model_id/compile` - authenticated semantic model SQL compilation.
- `GET /workbooks/blueprints` - spreadsheet/workbook and self-service query surfaces.
- `GET /dashboards/panels` - dashboard panel catalog for business, observability, and programmatic
  visualizations.
- `GET /renderers/contracts` - D3, Plotly/Dash, Evidence, and Office renderer/export contracts.
- `POST /etl/plans` - authenticated Domo Magic ETL/Power Query-style flow validation with lineage,
  materialization, and connector pushdown hints.
- `POST /diagrams/infra` - authenticated Terraform/AWS/GCP infrastructure diagram generation,
  including Terraform plan JSON, AWS Resource Explorer, GCP Cloud Asset, and multi-renderer graph
  export targets.
- `GET /reports/evidence` - Evidence.dev-style Markdown plus SQL report blueprint.
- `GET /security/policy` - hardening controls, limits, and residual-risk report.
- `GET /security/rbac` - role and permission policy for protected routes.
- `GET /schema` - request/response contract summary.
- `GET /example` - sample dataset, query, visualization, and evolution payloads.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe.
- `GET /metrics` - Prometheus text metrics.
- `POST /datasets` - authenticated dataset ingest.
- `GET /datasets` - authenticated dataset catalog.
- `GET /datasets/:dataset_id` - authenticated dataset profile.
- `GET /associations/:dataset_id` - authenticated Qlik-style associative graph over categorical
  fields.
- `POST /associations/select` - authenticated Qlik-style multi-dataset associative selection state.
- `POST /dashboards` - authenticated saved dashboard create/replace.
- `GET /dashboards` - authenticated saved dashboard catalog.
- `GET /dashboards/:dashboard_id` - authenticated saved dashboard definition.
- `POST /questions` - authenticated Metabase-style saved question create/replace with optional chart
  binding.
- `GET /questions` - authenticated saved question catalog.
- `POST /questions/nl` - authenticated natural-language prompt to self-service question proposals.
- `GET /questions/suggestions/:dataset_id` - authenticated dataset-aware question suggestions.
- `GET /questions/:question_id` - authenticated saved question definition.
- `GET /charts` - authenticated saved chart catalog derived from saved questions.
- `POST /alerts/rules` - authenticated Grafana-style alert rule create/replace.
- `GET /alerts/rules` - authenticated alert rule catalog.
- `GET /alerts/rules/:rule_id` - authenticated alert rule definition.
- `POST /alerts/rules/:rule_id/evaluate` - authenticated alert rule evaluation.
- `POST /alerts/contact-points` - authenticated alert contact point create/replace using secretRef
  destinations.
- `GET /alerts/contact-points` - authenticated alert contact point catalog.
- `GET /alerts/contact-points/:contact_id` - authenticated alert contact point definition.
- `POST /alerts/notification-policies` - authenticated alert notification policy create/replace.
- `GET /alerts/notification-policies` - authenticated alert notification policy catalog.
- `POST /alerts/rules/:rule_id/notification-preview` - authenticated dry-run notification delivery
  preview.
- `POST /query` - authenticated query translation and execution with an in-memory cache snapshot.
- `GET /query-cache` - authenticated query result cache summaries without raw query text.
- `GET /query-cache/:cache_id` - authenticated cached query result snapshot.
- `POST /sql-lab/history` - authenticated bounded SQL Lab history create with local execution or
  external dry-run planning.
- `GET /sql-lab/history` - authenticated SQL Lab history summaries without raw query text.
- `GET /sql-lab/history/:history_id` - authenticated SQL Lab history detail.
- `POST /visualizations/suggest` - authenticated visualization spec synthesis.
- `POST /evolution/run` - authenticated evolutionary visualization search.
- `GET /evolution/runs` - authenticated in-memory evolution run ledger.
- `POST /presentations/export` - authenticated PowerPoint/OpenXML, Google Slides, Reveal, and
  final-layer export.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json` - route-derived API docs.

Authenticated endpoints require `X-Server-Auth`, `Auth`, or `Authorization: Bearer ...` to match
`SERVER_AUTH_SECRET`, unless `DATA_VIZ_ALLOW_UNAUTHENTICATED=true` is set for local development.
Protected routes also enforce `X-Data-Viz-Role` / `X-DD-Role`; a valid operator request with no
role header defaults to `admin` for backward compatibility.

## Example flow

Start locally:

```sh
DATA_VIZ_ALLOW_UNAUTHENTICATED=true cargo run
```

Ingest a dataset:

```sh
curl -fsS http://127.0.0.1:8126/datasets \
  -H 'content-type: application/json' \
  -d '{
    "datasetId": "sales-lab",
    "displayName": "Sales Lab",
    "replace": true,
    "records": [
      {"region":"north","segment":"enterprise","revenue":1200,"margin":0.31,"churn":0.04,"latencyMs":44},
      {"region":"north","segment":"smb","revenue":760,"margin":0.24,"churn":0.08,"latencyMs":51},
      {"region":"south","segment":"enterprise","revenue":980,"margin":0.29,"churn":0.05,"latencyMs":47},
      {"region":"west","segment":"consumer","revenue":1320,"margin":0.19,"churn":0.11,"latencyMs":66}
    ]
  }'
```

Run SQL:

```sh
curl -fsS http://127.0.0.1:8126/query \
  -H 'content-type: application/json' \
  -d '{
    "dialect": "sql",
    "datasetId": "sales-lab",
    "query": "SELECT region, SUM(revenue) AS totalRevenue, AVG(margin) AS avgMargin FROM sales-lab GROUP BY region LIMIT 20"
  }'
```

Ask for a high-dimensional visualization:

```sh
curl -fsS http://127.0.0.1:8126/visualizations/suggest \
  -H 'content-type: application/json' \
  -d '{
    "datasetId": "sales-lab",
    "targetDimensions": 7,
    "intent": "compare revenue, margin, churn, and latency by region and segment"
  }'
```

Run evolutionary search with optional AI evaluator scores:

```sh
curl -fsS http://127.0.0.1:8126/evolution/run \
  -H 'content-type: application/json' \
  -d '{
    "datasetId": "sales-lab",
    "objective": "make executive-readable high-dimensional revenue risk views",
    "populationSize": 24,
    "generations": 8,
    "seed": 42,
    "aiEvaluations": [
      {
        "candidateId": "candidate-0",
        "score": 0.82,
        "rationale": "Readable comparison with a strong primary quantitative channel."
      }
    ]
  }'
```

## Presentation layers

`POST /presentations/export` returns:

- `powerpointOpenXml` - a PowerPoint OpenXML package file map that can be zipped into a `.pptx`
  artifact by a caller or future artifact worker.
- `googleSlidesBatchUpdate` - a Google Slides API `batchUpdate` blueprint.
- `revealMarkdown` - an open markdown deck representation.
- `finalLayers` - renderer-neutral JSON tying slides to visualization specs.

The service does not call Google APIs or write office files by itself in this first slice; it
emits deterministic presentation layers for downstream authenticated artifact workers.

## Runtime env

- `HOST` - default `0.0.0.0`.
- `PORT` - default `8126`.
- `SERVER_AUTH_SECRET` - operator/service auth secret.
- `DATA_VIZ_ALLOW_UNAUTHENTICATED` - local development bypass, default `false`.

Secrets belong in AWS Secrets Manager / Kubernetes secrets, not Git.

## Local checks

```sh
cargo fmt --check
cargo test
DATA_VIZ_ALLOW_UNAUTHENTICATED=true cargo run
curl -fsS http://127.0.0.1:8126/readyz
```

Build a container from the repo root:

```sh
docker build -f remote/deployments/dd-data-viz-rs/Dockerfile -t dd-data-viz-rs:dev .
```
