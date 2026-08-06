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

## Cross-surface delivery

User-visible, scenario, model, run, replay, result, authorization, notification,
navigation, or deep-link changes in this Rust web portal must be evaluated for:

- the planned Flutter app `akrion-sim/akrion-flutter` on Android, iOS, Flutter
  Web/mobile web, and Flutter desktop;
- the planned native Rust workbench `akrion-sim/akrion-desktop.rs`; and
- Akrion interfaces, generated clients, scenario/run schemas, deterministic
  seeds, route types, result bundles, and conformance fixtures.

This is judgment-based coordination. SEO, server-rendered portal presentation,
and web-only administration may remain web-only. Native local datasets,
large-batch execution, GPU visualization, file dialogs, and offline replay may
be native-specific. Scenario/model semantics, run state, deterministic replay,
result interpretation, permissions, errors, and navigation normally require
coordinated changes or an explicit no-change rationale and parity follow-up.

Deep links are HTTPS-first:

```text
https://<verified-akrion-owned-host>/open/<route>?<bounded-query>
```

A custom-scheme fallback must be assigned through a reviewed ADR before it is
registered; do not invent or ship an unowned scheme. Web, Flutter, and Rust
desktop must share versioned route types and fixtures and support cold start,
already-running delivery, authentication resume, replay/expiry rejection, and
browser fallback. Private datasets, result payloads, credentials, tokens,
absolute local paths, and sensitive simulation inputs are prohibited in URLs;
use bounded identifiers or short-lived, single-use, audience-bound handoff
codes.

See [`docs/CROSS_SURFACE_DELIVERY.md`](docs/CROSS_SURFACE_DELIVERY.md) and the
[portfolio policy](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md).
