# `remote/deployments/thread-fleet-exporter-go`

Tiny Go Prometheus exporter for the dd-dev thread fleet. Read-only by
construction — only `list`/`watch` on `Deployments`, `Pods`, and `PVCs` in `dd-dev`.

## Why this exists

The admin K8s dashboard at `/u/admin/k8s` already computes a thread fleet rollup
(`active|starting|sleeping|failed|dead`) by joining `Deployment` + `Pod` state. That logic
runs only when an admin opens the page, so Prometheus and Grafana have no visibility into the
fleet, and there are no alerts when, for example, half the threads slip into `failed`.

This exporter exposes the same taxonomy as Prometheus gauges so Grafana dashboards and
alert rules can see thread health without anyone opening the admin UI.

## Metrics

| Metric                                  | Type      | Labels                                                   |
| --------------------------------------- | --------- | -------------------------------------------------------- |
| `dd_thread_fleet_total`                 | gauge     | `phase` ∈ `active,starting,sleeping,failed,dead`         |
| `dd_thread_fleet_replicas_desired_total`| gauge     | —                                                        |
| `dd_thread_fleet_replicas_ready_total`  | gauge     | —                                                        |
| `dd_thread_fleet_pvcs_total`            | gauge     | `state` ∈ `bound,pending,lost,unknown`                   |
| `dd_thread_fleet_threads`               | gauge (1) | `thread_id_short`, `thread_id`, `user_id`, `managed_by`  |
| `dd_thread_fleet_scrape_seconds`        | histogram | —                                                        |
| `dd_thread_fleet_scrape_errors_total`   | counter   | —                                                        |

The `managed_by` label is `dd-thread-operator` for resources owned by the operator and
`template` otherwise. That gives a clean Grafana split during a future migration from
template-provisioned to operator-managed threads.

`dd_thread_fleet_threads` cardinality is bounded by the number of live thread Deployments
(low-tens today, hundreds in steady state). Reset on every scrape so deleted threads drop out
of the metric set.

## Scope

- Namespace: `dd-dev` (override via `--namespace` or `THREAD_FLEET_NAMESPACE`).
- Label selector: `app.kubernetes.io/component=thread-pod` — matches both the existing
  template-based path and the operator path.
- Scrape period: 15s (override via `--scrape-period`).

## Build + run locally

```bash
go vet ./...
go test ./...
go build -o /tmp/dd-thread-fleet-exporter ./cmd/exporter
KUBECONFIG=$HOME/.kube/config /tmp/dd-thread-fleet-exporter --listen-addr=:9103
curl localhost:9103/metrics | grep dd_thread_fleet_total
```

## Cluster deploy (EC2)

Apply via Argo CD — the `Application` manifest is at
[`remote/argocd/apps/dd-thread-fleet-exporter.application.yaml`](../../argocd/apps/dd-thread-fleet-exporter.application.yaml).

Once running, scrape configuration depends on whether you use the OpenTelemetry Collector or
Prometheus directly:

- **OTel Collector** (current default): add a static target
  `dd-thread-fleet-exporter.default.svc.cluster.local:9103` to the collector's Prometheus
  receiver, same as `dd-remote-rest-api` and `dd-remote-web-home`.
- **Prometheus**: add a static `scrape_configs` entry with the same target.

There is no `ServiceMonitor` here because the cluster runs Prometheus directly, not the
Operator-flavoured stack.
