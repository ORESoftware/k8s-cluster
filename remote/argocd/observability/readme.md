# `remote/argocd/observability`

GitOps-managed observability stack for the EC2 Kubernetes cluster.

## Components

- `dd-otel-collector`: receives OTLP traces and scrapes runtime `/metrics`
  endpoints.
- `dd-prometheus`: stores collector-exported metrics.
- `dd-grafana`: serves dashboards at `/telemetry/` through the public gateway.
- `dd-loki` + `dd-promtail`: collect Kubernetes container stdout/stderr logs
  from `/var/log/containers/*.log`.
- `dd-tempo` and `dd-jaeger`: trace backends for collector-exported spans.
- `dd-nats` scrape target: NATS metrics are collected from the exporter sidecar
  at `dd-nats.messaging.svc.cluster.local:7777`.

The runtimes are instrumented explicitly:

- Node worker/API emits direct OTLP/HTTP spans and Prometheus metrics.
- Rust web-home emits Prometheus metrics.
- Rust REST API emits Prometheus metrics for the RDS/Postgres data boundary.
- Gleam websocket server emits actor-backed Prometheus metrics.
- Gleam MCP server emits HTTP and JSON-RPC method Prometheus metrics.
- Akka/async.java websocket server emits Prometheus counters for both
  websocket pipelines.
- F# websocket server exposes its Rx live counters as Prometheus metrics.
- Spark pipeline server, formal-methods server, and the GCS router are scraped
  directly by the collector from their `/metrics` endpoints.
- Auth, agent-worker broker, billing, formal-methods-service, and lock
  load-test trigger expose lightweight Prometheus health/work counters.
- Rust MDP optimizer emits Prometheus metrics and accepts compact app/infra
  telemetry snapshots on `/mdp/telemetry/learn` or `dd.remote.telemetry.mdp`
  for policy learning over operational risk.
- NATS emits server, connection, subscription, and JetStream metrics through
  `natsio/prometheus-nats-exporter`.

Node does not use OpenTelemetry auto-instrumentation or monkey-patching.
