<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/soccer-rs` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/akrion-sim/akrion-backend.rs` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/soccer-rs`.
<!-- END k8s-cluster-submodule-notice -->

# akrion-backend.rs

Root Akrion backend: the Rust HTTP server (crate `dd-soccer-rs`) for the Akrion
simulation. UUID-keyed live games over the `soccer_engine` simulation library,
plus the browser-rendered mermaid docs site at `/soccer/docs`.

Extracted from `k8s-cluster/remote/deployments/soccer-rs`, and wired back into
that GitOps repo as a **git submodule** at `remote/deployments/soccer-rs`
(mirrors how `sonus-auris-backend.rs` lives at
`remote/deployments/dd-sound-recorder-rs`).

## Building

This crate path-depends on the simulation engine:

```toml
soccer_engine = { path = "../akrion-soccer-engine.rs" }
```

which itself path-depends on `des_engine` (`discrete-event-system.rs`). That
relative path resolves only when the engine sources sit adjacently, as they do
in this Akrion workspace:

- `akrion-backend.rs`
- `akrion-soccer-engine.rs`
- `discrete-event-system.rs`

## Service structure

The binary entry point is intentionally thin. Runtime responsibilities are split
into focused modules:

- `src/main.rs` — Tokio bootstrap only.
- `src/config.rs` — live-learning environment parsing and engine configuration.
- `src/database.rs` — optional SeaORM/Postgres connection and readiness state.
- `src/docs.rs` — embedded docs and the mounted live UI.
- `src/server.rs` — game/session state, HTTP/WebSocket routes, and bind lifecycle.
- `src/telemetry.rs` — structured logs, OTLP traces/metrics, and HTTP instrumentation.

New application persistence must use the `DatabaseConnection` exposed by
`src/database.rs`. Do not add a direct `sqlx` dependency or call SQLx APIs from
service code. SeaORM uses SQLx internally as its Postgres driver, so SQLx entries
in `Cargo.lock` are expected transitive implementation details.

`AKRION_DATABASE_URL` enables the service database connection. When it is unset,
the database is explicitly disabled and `/readyz` remains healthy. When it is set,
`/readyz` pings the SeaORM pool and returns `503` if it is unavailable.

A bare standalone clone of this repo will not build on its own — assemble the
engine sources adjacently first (same as before extraction).

## Telemetry

The server initializes tracing and OpenTelemetry at startup. Text logs remain the
local default. Cluster deployments emit JSON to stdout for Promtail/Loki and send
OTLP/HTTP traces and metrics to the collector; Prometheus scrapes the collector's
OTLP Prometheus exporter. HTTP counters and duration histograms use route templates,
never game ids or raw URLs, to keep label cardinality bounded.

| Variable | Default | Purpose |
| --- | --- | --- |
| `AKRION_TELEMETRY_ENABLED` | `true` | Enables the tracing subscriber. |
| `AKRION_LOG_JSON` | `false` | Emits newline-delimited JSON logs to stdout. |
| `AKRION_RUST_LOG` | `info,dd_soccer_rs=info,tower_http=info` | Service-specific tracing filter; falls back to `RUST_LOG`. |
| `AKRION_SERVICE_NAME` | `dd-soccer-rs` | Service name in logs and OTel resources. |
| `AKRION_CLUSTER_NAME` | unset/local | Cluster resource attribute. |
| `AKRION_OTEL_TRACES` | endpoint-driven | Enables OTLP trace export. |
| `AKRION_OTEL_METRICS` | endpoint-driven | Enables OTLP metric export. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset | Generic OTLP/HTTP base; `/v1/traces` and `/v1/metrics` are derived automatically. |
| `AKRION_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | `http://127.0.0.1:4318/v1/traces` | OTLP/HTTP trace endpoint. |
| `AKRION_OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | `http://127.0.0.1:4318/v1/metrics` | OTLP/HTTP metrics endpoint. |

## Routes

See `src/server.rs` — `/soccer/game|live|sim|inspect`, `/soccer/api/*`,
`/soccer/docs` (public mermaid docs), `/healthz` (liveness), and `/readyz`
(SeaORM readiness when configured).
