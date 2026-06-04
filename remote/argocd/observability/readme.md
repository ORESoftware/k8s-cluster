# `remote/argocd/observability`

GitOps-managed observability stack for the EC2 Kubernetes cluster.

## Components

- `dd-otel-collector`: receives OTLP traces and scrapes runtime `/metrics`
  endpoints.
- `dd-prometheus`: stores collector-exported metrics plus direct service
  scrapes for observability, messaging, and selected runtime endpoints.
- `dd-k8s-resource-exporter`: exposes bounded Kubernetes Deployment,
  StatefulSet, DaemonSet, pod, container-resource, event, and node
  saturation metrics for the checked-in workload allowlist, plus GCS
  dependency TCP probes, Redis INFO samples, and RabbitMQ management
  overview samples.
- `dd-grafana`: serves dashboards at `/telemetry/` through the public gateway.
  Includes the `GCS WSS Load Collapse` dashboard for 10k/20k chat.vibe load
  tests.
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
- GCS chat.vibe exports per-pod WSS/REST active connections, broker publish
  rates/latencies, Kafka read errors, broker subscriptions, Go runtime state,
  and Linux process CPU/RSS/fd gauges for load-collapse diagnosis.
- The Kubernetes resource exporter adds the evidence the app cannot expose
  after a pod dies: desired/available replicas, restart counts, last
  termination reasons such as `OOMKilled`, metrics-server CPU/memory usage
  versus container limits, warning events, node saturation, TCP reachability
  to Kafka/RabbitMQ/MongoDB/Redis, selected Redis INFO values, and RabbitMQ
  connection/channel/queue/message counters.
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
- `dd-k8s-resource-exporter` -> `dd-k8s-resource-exporter`

### Per-pod metrics + log labels

- `dd-prometheus` scrapes Grafana at `/telemetry/metrics` to match the
  gateway subpath config, and scrapes Loki directly at `/metrics`. Keep
  those as explicit jobs so Grafana target health and Loki ingestion
  health stay independently visible.
- `dd-otel-collector` uses `kubernetes_sd_configs (role: pod)` for the
  `gcs-router` and `dd-promtail` scrape jobs so each pod is scraped
  directly. The collector exports `gcs_router_*` counters with a `pod`
  label, which is needed because the per-pod ring counters disagree
  (each pod tracks routing decisions from its own perspective) and the
  Service VIP would hide half the signal behind round-robin scraping.
  Promtail's own `/metrics` is scraped the same way so empty-Loki
  incidents can be diagnosed from Prometheus.
- The `Kubernetes Workload Fleet` Grafana dashboard (uid
  `dd-kubernetes-workload-fleet`) is the generic dashboard for every
  checked-in workload in the exporter allowlist. It is driven by
  `dd_k8s_workload_*_replicas`, pod restart/resource metrics, Kubernetes
  event metrics, and Loki deployment labels, with a repeatable `workload`
  variable for per-workload panels. Keep `WATCH_NAMESPACES` and
  `WATCH_APPS` aligned with checked-in Deployment, StatefulSet, and
  DaemonSet manifests when adding or removing services.
- The `Deployment Drilldown` Grafana dashboard (uid
  `dd-deployment-drilldown`) is the canonical per-service page. The Rust
  web-home deployment redirects `/grafana/depl/<deployment>` into this
  dashboard with `var-deployment=<deployment>`, so paths such as
  `/grafana/depl/dd-dart-server`, `/grafana/depl/dd-billing-server`, and
  `/grafana/depl/des-rs` land on the same Prometheus/Loki-backed view with
  that service preselected. Run
  `node remote/tools/check-observability-coverage.mjs` after adding or
  removing manifests to verify every checked-in workload is watched, the
  Grafana route stays provisioned, and deployment dependency manifests do not
  add forbidden auto-instrumentation or monkey-patching packages.
- `dd-promtail` tails the stable `/var/log/containers/*.log` symlink
  farm directly via `static_configs` (no Kubernetes API dependency): the
  service-discovery informer once found zero targets in this EC2 cluster
  even though API reachability, the mounted ServiceAccount token, RBAC,
  and TLS were all verified healthy, so the static file glob is the
  reliable source of log streams. The `cri` pipeline stage decodes the
  containerd envelope, and the log filename is parsed into `namespace`,
  `pod`, and `container` stream labels plus a per-deployment
  `app`/`deployment` label (the ReplicaSet hash + pod suffix are
  stripped). A literal `cluster=dd-ec2` external label is stamped on the
  push client — promtail deliberately runs WITHOUT
  `-config.expand-env=true`, since that expander collides with the `$`
  end-anchors in the pipeline regexes. Every stream defaults to
  `env=stage`/`environment=stage`; known production deployments
  (`dd-billing-server`, `dd-web-scraper`, `dd-browser-test-server`) are
  promoted to `env=prod`/`environment=prod` by a `match` stage. Loki
  queries should pin on these labels, e.g.
  `{namespace="default", deployment="dd-dart-server"}` or
  `{environment="prod"}`, rather than regexing `filename`, which Promtail
  drops after parsing to avoid per-restart stream cardinality.

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

  Stage vs prod is a per-deployment label, not a per-cluster one (this
  EC2 cluster runs both): streams default to `env=stage` and the `match`
  stage promotes the known production deployments to `env=prod`. To
  onboard another prod service, add it to that selector in
  `promtail.configmap.yaml`. A genuinely separate future cluster would
  instead set a distinct literal `cluster` external label so a shared or
  proxied Loki can tell the two clusters apart.

  Promtail also opportunistically parses the shared `dd.log.v1` JSON
  stdout/stderr envelope from `docs/observability-stdio-contract.md`.
  Only low-cardinality fields are promoted to labels (`log_schema`,
  `severity`, and `log_service`); request ids, task ids, thread ids,
  trace/span ids, paths, messages, and error details remain log fields.

The OTEL collector keeps its own minimal RBAC
(`otel-collector.rbac.yaml`) for `kubernetes_sd` pod discovery.
`promtail.rbac.yaml` is retained (read-only pods) for optional metadata
enrichment, but the current filename-based pipeline does not require the
Kubernetes API.
