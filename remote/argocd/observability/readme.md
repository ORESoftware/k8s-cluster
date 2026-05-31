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

## Operator notes

### `reloader` (`reloader.deployment.yaml`)

`stakater/reloader` watches ConfigMaps + Secrets cluster-wide. Any
controller (Deployment / StatefulSet / DaemonSet) that opts in via

```yaml
metadata:
  annotations:
    configmap.reloader.stakater.com/reload: "<name-of-configmap>"
```

gets a rolling restart whenever the named configmap's data changes.

This replaces the previous manual pattern of bumping
`dd.dev/config-revision` on the pod template, which had a sharp edge:
if you forgot to bump the annotation while editing a configmap, Argo
CD synced the configmap but the dependent pods kept running with the
stale config until somebody noticed (this happened to
`dd-otel-collector` and silently dropped the `gcs-router` scrape
target until a manual `kubectl rollout restart` recovered it).

Currently opted-in:

- `dd-otel-collector` -> `dd-otel-collector-config`
- `dd-prometheus`     -> `dd-prometheus-config`
- `dd-promtail`       -> `dd-promtail-config`

### Per-pod metrics + log labels

- `dd-otel-collector` uses `kubernetes_sd_configs (role: pod)` for the
  `gcs-router` scrape job so each router pod is scraped directly. The
  collector exports `gcs_router_*` counters with a `pod` label, which
  is needed because the per-pod ring counters disagree (each pod
  tracks routing decisions from its own perspective) and the Service
  VIP would hide half the signal behind round-robin scraping.
- `dd-promtail` tails `/var/log/containers/*.log` directly via
  `static_configs` (no Kubernetes API dependency), decodes the
  containerd envelope with the `cri` pipeline stage, and lifts
  `namespace`, `pod`, and `container` out of the log filename into
  first-class stream labels. The push client also stamps agent-scoped
  `cluster`, `env`, and `node` labels (from `CLUSTER_NAME`,
  `CLUSTER_ENV`, and the downward-API `NODE_NAME`, expanded with
  `-config.expand-env=true`). Loki queries should pin on these labels,
  e.g. `{env="prod", namespace="default", container="dart-server"}`.
  The high-cardinality auto `filename` label is dropped after parsing.

  Note: an earlier revision moved promtail to
  `kubernetes_sd_configs (role: pod)` with a
  `/var/log/pods/*$1/*.log` glob; that left Loki with no streams and
  no labels. The DaemonSet now also tolerates all node taints and runs
  as root (`runAsUser: 0`) so it schedules on every node and can read
  the root-owned container log files. Promtail's positions file lives on a
  `hostPath` (`/var/lib/dd-promtail`) rather than an `emptyDir`, so a restart
  resumes from the last read offset instead of re-reading every container log
  from the start and replaying old rotated lines that Loki rejects as
  `timestamp too old`.

  To distinguish a future stage cluster from prod, deploy promtail with
  `CLUSTER_ENV=stage` (and a distinct `CLUSTER_NAME`); the per-cluster
  `env`/`cluster` labels then separate the two log sources in a shared
  or proxied Loki.

The OTEL collector keeps its own minimal RBAC
(`otel-collector.rbac.yaml`) for `kubernetes_sd` pod discovery.
`promtail.rbac.yaml` is retained (read-only pods) for optional metadata
enrichment, but the current filename-based pipeline does not require the
Kubernetes API.
