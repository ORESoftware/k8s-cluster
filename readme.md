# akrion-web-server.rs

Rust web portal for Akrion Sim.

This repository is the canonical source. `~/codes/ores/k8s-cluster` consumes it
as the secondary submodule `remote/deployments/akrion-web-server-rs`; make code
changes here and advance the GitOps gitlink after the app commit is published.

- `akrion-backend.rs` owns realtime game/simulation routes.
- `akrion-web-server.rs` owns web pages, portal UI, htmx fragments, WebSocket stats, and Supabase browser login.

## Layout

- `src/main.rs` is the thin process bootstrap and socket lifecycle.
- `src/app.rs` owns app state, public browser config, environment helpers, and static asset paths.
- `src/database.rs` owns the optional SeaORM/Postgres pool and readiness state.
- `src/routes.rs` owns axum routes, partial handlers, and the htmx WebSocket stream.
- `src/shutdown.rs` owns cross-platform graceful shutdown signals.
- `src/telemetry.rs` owns JSON logs, OTLP/HTTP traces and metrics, and bounded HTTP instrumentation.
- `src/views.rs` owns maud page templates and reusable UI fragments.
- `src/data.rs` owns dashboard stats and placeholder portal rows.
- `assets/app.css` and `assets/app.js` own the browser UI layer.

## Run

```bash
PORT=8124 AKRION_BACKEND_URL=http://127.0.0.1:8113 cargo run
```

Then open `http://127.0.0.1:8124`.

`/healthz` is process liveness. `/readyz` checks the SeaORM pool when
`AKRION_DATABASE_URL` is configured; without a database it reports `database
disabled` and remains ready. New persistence code must use SeaORM through
`DatabaseState`; do not add direct SQLx dependencies or SQLx API calls. SQLx
entries in `Cargo.lock` are expected because it is SeaORM's internal driver.

To mount the app behind a path prefix, set `AKRION_WEB_BASE_PATH`:

```bash
PORT=8124 AKRION_WEB_BASE_PATH=/akrion-sim cargo run
```

## Supabase Login

The page enables Supabase auth when both values are present:

```bash
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_ANON_KEY=your-public-anon-key
```

Only the public anon key is emitted to the browser. Do not put a Supabase service-role key in this server's public config.

## Observability

For Kubernetes, set `AKRION_LOG_JSON=true` so Promtail can send structured
stdout logs to Loki, and set `OTEL_EXPORTER_OTLP_ENDPOINT` to the collector's
OTLP/HTTP base URL. The server derives `/v1/traces` and `/v1/metrics`, exports
both signals, and records request count, active requests, and duration using
route templates rather than raw URLs.
