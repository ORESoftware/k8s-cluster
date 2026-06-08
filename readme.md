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

Each frontend currently implements a useful analytics subset: source selection, simple filters,
grouping, and `count`/`sum`/`avg`/`min`/`max` aggregations where the dialect naturally supports
them.

## Platform parity map

The codebase now has dedicated platform and hardening modules instead of putting every product
concept in `main.rs`:

- `src/platform.rs` defines parity surfaces for Tableau, Power BI, Qlik Sense, Looker, Sigma,
  Domo, Superset, Metabase, Grafana, D3.js, Plotly/Dash, and Evidence.dev.
- `src/hardening.rs` defines operator auth posture, input limits, implemented controls, and
  residual risks.

Current first-class parity surfaces:

- Power BI / Looker: governed semantic model descriptors with dimensions, measures, DAX analogs,
  and calculated fields.
- Qlik Sense: `GET /associations/:dataset_id` builds a categorical co-occurrence graph over an
  ingested dataset.
- Sigma: workbook blueprints for live-grid and executive-card workflows.
- Domo / Power Query: connector catalog plus ETL planner primitives.
- Superset / Metabase: SQL lab and visual query-builder/self-service contracts.
- Grafana: time-series dashboard panel catalog, PromQL/LogQL query frontends, and metrics route.
- D3.js / Plotly / Dash / Evidence.dev: renderer contracts, final-layer JSON, Plotly trace
  blueprint posture, and Markdown-plus-SQL report blueprint.

## Endpoints

- `GET /` - HTML operator home.
- `GET /descriptor` - service descriptor, storage model, dialect catalog, and route map.
- `GET /dialects` - query dialect catalog.
- `GET /capabilities/parity` - BI and visualization tool parity matrix.
- `GET /connectors/catalog` - connector catalog and ETL planner primitives.
- `GET /semantic/models` - governed semantic models, dimensions, measures, and calculations.
- `GET /workbooks/blueprints` - spreadsheet/workbook and self-service query surfaces.
- `GET /dashboards/panels` - dashboard panel catalog for business, observability, and programmatic
  visualizations.
- `GET /renderers/contracts` - D3, Plotly/Dash, Evidence, and Office renderer/export contracts.
- `GET /reports/evidence` - Evidence.dev-style Markdown plus SQL report blueprint.
- `GET /security/policy` - hardening controls, limits, and residual-risk report.
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
- `POST /query` - authenticated query translation and execution.
- `POST /visualizations/suggest` - authenticated visualization spec synthesis.
- `POST /evolution/run` - authenticated evolutionary visualization search.
- `GET /evolution/runs` - authenticated in-memory evolution run ledger.
- `POST /presentations/export` - authenticated PowerPoint/OpenXML, Google Slides, Reveal, and
  final-layer export.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json` - route-derived API docs.

Authenticated endpoints require `X-Server-Auth`, `Auth`, or `Authorization: Bearer ...` to match
`SERVER_AUTH_SECRET`, unless `DATA_VIZ_ALLOW_UNAUTHENTICATED=true` is set for local development.

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
