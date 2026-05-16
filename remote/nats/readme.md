# NATS in the remote cluster

This directory is a documentation landing page for the remote NATS setup. The
live Kubernetes resources are currently defined elsewhere so ArgoCD can manage
them directly.

## Source of truth

- ArgoCD application:
  [`remote/argocd/apps/dd-messaging.application.yaml`](../argocd/apps/dd-messaging.application.yaml)
- NATS manifests:
  [`remote/argocd/messaging/`](../argocd/messaging/)
- Observability scrape and dashboards:
  [`remote/argocd/observability/`](../argocd/observability/)
- Public gateway paths:
  [`remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml`](../argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml)

`remote/k8s` does not currently declare NATS. The intended production path is
the ArgoCD `dd-messaging` application, which watches the `dev` branch and syncs
`remote/argocd/messaging` into the `messaging` namespace.

## Runtime components

- `dd-nats`: NATS server with JetStream enabled.
- `prometheus-exporter`: `natsio/prometheus-nats-exporter` sidecar for metrics.
- `dd-remote-rest-api`: configured with
  `NATS_URL=nats://dd-nats.messaging.svc.cluster.local:4222`.

## Resource sizing

CPU and RAM are defined in
[`remote/argocd/messaging/nats.deployment.yaml`](../argocd/messaging/nats.deployment.yaml):

- `nats` container: requests `100m` CPU / `128Mi` RAM, limits `1` CPU /
  `1Gi` RAM.
- `prometheus-exporter` sidecar: requests `50m` CPU / `64Mi` RAM, limits
  `500m` CPU / `256Mi` RAM.

JetStream storage is configured in
[`remote/argocd/messaging/nats.configmap.yaml`](../argocd/messaging/nats.configmap.yaml):

- `store_dir`: `/data/jetstream`
- `max_mem_store`: `1GB`
- `max_file_store`: `20GB`

The backing volume is mounted from the EC2 host path `/var/lib/dd/nats`, defined
in the Deployment. That host path does not enforce a Kubernetes volume quota by
itself; the effective disk ceiling is the NATS `max_file_store` setting plus
available EC2 disk.

## In-cluster endpoints

- Client URL: `nats://dd-nats.messaging.svc.cluster.local:4222`
- Monitoring URL: `http://dd-nats.messaging.svc.cluster.local:8222`
- Metrics URL: `http://dd-nats.messaging.svc.cluster.local:7777/metrics`

## Public inspection paths

These are routed through the runtime gateway on the EC2 ingress:

- `/nats/`
- `/nats-metrics/metrics`

Grafana is still the preferred way to inspect NATS health because it combines
connection count, throughput, server resource usage, JetStream activity, and
slow-consumer metrics on one dashboard.

## Storage note

JetStream data currently uses the EC2 host path `/var/lib/dd/nats`, which is
fine for the current single-node cluster. Before moving to a multi-node cluster,
replace that host path with a real Kubernetes storage class or managed volume.
