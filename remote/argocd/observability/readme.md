# `remote/argocd/observability`

GitOps-managed observability stack for the EC2 Kubernetes cluster.

## Components

- `dd-otel-collector`: receives OTLP traces and scrapes runtime `/metrics`
  endpoints.
- `dd-prometheus`: stores collector-exported metrics plus direct service
  scrapes for observability, messaging, and selected runtime endpoints. It
  also evaluates `observability.rules.yml` for target-health, workload, and
  collector-flow alerts.
- `dd-k8s-resource-exporter`: exposes bounded Kubernetes Deployment,
  StatefulSet, DaemonSet, pod, container-resource, event, and node
  saturation metrics for the checked-in workload allowlist, plus GCS
  dependency TCP probes, Redis INFO samples, and RabbitMQ management
  overview samples.
- `dd-grafana`: serves dashboards at `/telemetry/` through the public gateway.
  Includes the `Observability Control Plane`, `Deployment Drilldown`,
  `Kubernetes Workload Fleet`, `Fabrication Planner`, and `GCS WSS Load
  Collapse` dashboards.
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
- The Solana contract gateway exposes Prometheus counters for validation,
  policy rejection, Solana RPC method outcomes, NATS publish outcomes, and
  send-auth failures, plus `dd.log.v1` stdout/stderr records for Loki.
- Runtime-config exposes subscriber, entry, and push counters that make
  configuration delivery visible for dependent planners such as
  `dd-fabrication-server`; Prometheus alerts when the target is down, stage
  subscribers disappear, or stage push errors increase.
- Rust MDP optimizer emits Prometheus metrics and accepts compact app/infra
  telemetry snapshots on `/mdp/telemetry/learn` or `dd.remote.telemetry.mdp`
  for policy learning over operational risk. Prometheus alerts when the
  optimizer target is down because fabrication planning depends on it for
  policy optimization fan-out.
- NATS emits server, connection, subscription, and JetStream metrics through
  `natsio/prometheus-nats-exporter`; Prometheus alerts when the NATS scrape
  target is down because fabrication queue intake, results, runtime events,
  and learning fan-out depend on it.

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
- `dd-loki`           -> `dd-loki-config`

### Control-plane health and guardrails

- `/grafana/observability` redirects from the Rust web-home service to the
  `Observability Control Plane` dashboard (uid
  `dd-observability-control-plane`). Use it first when Grafana, Prometheus,
  Loki, Promtail, or the collector may be part of the incident.
- `dd-prometheus` loads `/etc/prometheus/observability.rules.yml`, recording
  `dd:observability:target_up_ratio` and
  `dd:k8s_workload:available_ratio`, and raising DD-prefixed alerts for down
  observability targets, missing Promtail targets, unavailable workloads,
  resource-exporter failure, and collector refused/export-failed telemetry.
- `dd-otel-collector` exposes explicit self telemetry at
  `dd-otel-collector.observability.svc.cluster.local:8888` and a
  `health_check` endpoint at `:13133`. Kubernetes liveness/readiness probes
  use the health extension; Prometheus separately scrapes the collector's
  self metrics as `otel-collector-self` while keeping the pipeline exporter at
  `otel-collector` on `:8889`. The pipeline exporter scrape uses
  `honor_labels: true` so collector-exported service and pod scrape labels
  remain queryable as their original `job`, `instance`, `pod`, and `app`
  dimensions instead of being renamed behind the collector target label.
- `dd-loki` keeps single-node filesystem storage, but bounds ingestion and
  query cost with `limits_config`: old samples are rejected after seven days,
  ingestion bursts are capped, query splitting/parallelism is bounded, and
  label count/length limits protect stream cardinality.
- `dd-promtail` keeps the static `/var/log/containers/*.log` pattern but now
  has explicit push batching, timeout, and retry backoff settings so Loki
  slowness does not create unbounded client behavior.

### Per-pod metrics + log labels

- `dd-prometheus` scrapes Grafana at `/telemetry/metrics` to match the
  gateway subpath config, and scrapes Loki directly at `/metrics`. Keep
  those as explicit jobs so Grafana target health and Loki ingestion
  health stay independently visible.
- `dd-otel-collector` uses `kubernetes_sd_configs (role: pod)` for the
  `gcs-router`, `dd-fabrication-server-pods`, and `dd-promtail` scrape jobs so each pod is scraped
  directly. The collector exports `gcs_router_*` counters with a `pod`
  label, which is needed because the per-pod ring counters disagree
  (each pod tracks routing decisions from its own perspective) and the
  Service VIP would hide half the signal behind round-robin scraping.
  Fabrication server pod scraping preserves replica-local job/artifact
  ledger, learning-memory, failure-boundary, and NATS/MDP fan-out counters
  that would otherwise be hidden by the Service VIP and `ClientIP` affinity.
  Prometheus alerts when the `dd-fabrication-server-pods` target set is
  absent or an individual pod target stays down, so a broken direct scrape
  cannot quietly mask one replica's retained fabrication evidence.
  A separate direct pod scrape coverage alert fires when Prometheus sees
  fewer ready direct pod scrapes than desired replicas, catching partial
  discovery or collector-export gaps before the service scrape makes the
  planner look healthier than its replica-local evidence really is.
  The `Fabrication Planner` Grafana dashboard (uid `dd-fabrication-planner`)
  groups those signals with request intake, validation-finding and
  machine-failure boundary rates, required operator-action, fixture/setup
  blocker, and split/combine review rates, NATS queued request ingest, all-publish attempts,
  panel legends for validation-finding, machine-failure boundary, required operator-action, fixture/setup blocker, and
  split/combine review rates,
  a composite release-readiness blocker rate, intervention/automation review
  pressure,
  result fanout, MDP optimization fanout, generated-program,
  job/artifact, learning-event, and artifact detail-request throughput, in-memory
  job/artifact/learning evidence ledgers, including an artifact high-watermark
  alert for retained design, machine-code, and instruction evidence,
  runtime-config push delivery, dependency scrape health, HPA capacity, CPU and
  memory limit headroom, Loki-derived gateway guardrail rejection counters for
  `/fabrication` auth/internal-route/method/payload/rate-limit failures, gateway edge-latency
  p95/max panels from redacted access-log `request_time`, upstream p95/max panels from
  `upstream_response_time`, upstream 500/502/503/504 failure counters from
  `upstream_status`, request-size p95/max panels from `request_length` so payload growth is visible
  before the `512k` gateway cap returns 413s, response-size p95/max panels from
  `body_bytes_sent` for generated design, machine-code, and artifact responses, gateway access/guardrail logs, and
  warning/error logs for the Rust planner. Its direct pod scrape coverage
  panel compares ready `dd-fabrication-server-pods` scrapes with desired
  Deployment replicas and shows the scrape coverage gap.
  The gateway log panel reads the redacted `dd.gateway.access.v1` access-log
  lines, which include request IDs, statuses, upstream status/timing, and
  path-only URIs without Auth headers, cookies, or query strings.
  Fabrication also has a service-scoped workload-availability alert using
  `dd:k8s_workload:available_ratio` plus a rollout-lag alert on updated
  Deployment replicas, so cold release builds, scheduling pressure, readiness
  failures, or a partially rolled out hardened planner show up under
  `dd-fabrication-server` and not only the generic Kubernetes workload alert.
  The same k8s-resource
  exporter metrics alert on serving-container restarts and waiting states,
  because restarts or stuck runtime startup can interrupt retained job/artifact
  evidence, learning memory, NATS subscriptions, and active planning work.
  The exporter also emits
  init-container waiting/restart metrics so fabrication alerts can distinguish
  source-layout validation or release-build startup failures from running
  service failures. CPU and memory near-limit alerts use the same exporter
  resource gauges, because sustained saturation can delay instruction analysis,
  result fanout, and learning feedback even when the scrape target stays up.
  Separate intervention/setup alerts fire when the Rust server starts emitting
  required operator actions, fixture/setup blockers, or split/combine review
  records, because those counters mean generated or imported fabrication work is
  explicitly not machine-ready until human, workholding, decomposition,
  recomposition, or interface-control evidence is resolved. A composite
  release-readiness alert also groups machine-failure boundaries, operator
  actions, fixture/setup blockers, and split/combine reviews so operators see a
  single service-scoped machine-release hold while the detailed blocker alerts
  remain available for triage. A separate intervention/automation review alert
  groups operator actions with split/combine reviews so automation candidates,
  human handoffs, `interventionMap`, `operatorInterventionPlan`, and
  `executionPlan` evidence stay visible before unattended machine release. A separate
  queued-NATS fanout alert fires when the Rust server consumes
  `dd_fabrication_server_nats_messages_total` traffic but stops increasing
  `dd_fabrication_server_nats_results_published_total`, so queue-worker
  regressions are visible even without concurrent HTTP requests.
  It also emits watched HPA current/desired/min/max replica
  gauges plus an at-max signal, and Prometheus alerts when the
  `dd-fabrication-server` autoscaler holds at its configured ceiling during
  planning or instruction-analysis pressure. Runtime-config push failures for the `dd-fabrication-server`
  subscriber are also alerted separately from the fleet-wide runtime-config
  push-error alert, because stale fabrication config can affect planning, NATS
  fan-out, and MDP learning behavior.
  Promtail's own `/metrics` is scraped the same way so empty-Loki
  incidents can be diagnosed from Prometheus.
- The `Kubernetes Workload Fleet` Grafana dashboard (uid
  `dd-kubernetes-workload-fleet`) is the generic dashboard for every
  checked-in workload in the exporter allowlist. It is driven by
  `dd_k8s_workload_*_replicas`, pod restart/resource metrics, Kubernetes
  event metrics, and Loki deployment labels, with a repeatable `workload`
  variable for per-workload panels. It also overlays watched HPA
  current/desired/max replica traces and at-max signals so autoscaler pressure
  is visible beside unavailable replicas, restarts, and waiting containers.
  Keep `WATCH_NAMESPACES` and
  `WATCH_APPS` aligned with checked-in Deployment, StatefulSet, and
  DaemonSet manifests when adding or removing services.
- The `Deployment Drilldown` Grafana dashboard (uid
  `dd-deployment-drilldown`) is the canonical per-service page. The Rust
  web-home deployment redirects `/grafana/depl/<deployment>` into this
  dashboard with `var-deployment=<deployment>`, so paths such as
  `/grafana/depl/dd-dart-server`, `/grafana/depl/dd-billing-server`, and
  `/grafana/depl/des-rs` land on the same Prometheus/Loki-backed view with
  that service preselected. Its replica panel overlays matching HPA
  current/desired/max replica traces, which makes
  `/grafana/depl/dd-fabrication-server` useful for spotting fabrication
  planner saturation before the service is fully unavailable. Run
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
  (`dd-billing-server`, `dd-web-scraper`, `dd-browser-test-server`,
  `dd-selenium-server`, `dd-browser-job-runner`) are
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
