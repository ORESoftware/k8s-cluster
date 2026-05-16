# `remote/argocd/messaging`

GitOps-managed messaging layer for remote-dev.

## Components

- `dd-nats`: NATS server with JetStream enabled.
- `prometheus-exporter` sidecar: `natsio/prometheus-nats-exporter`, exposing
  NATS metrics on `:7777`.

## In-cluster endpoints

- NATS client URL: `nats://dd-nats.messaging.svc.cluster.local:4222`
- NATS monitoring: `http://dd-nats.messaging.svc.cluster.local:8222`
- Prometheus metrics: `http://dd-nats.messaging.svc.cluster.local:7777/metrics`

Data is stored on the EC2 host at `/var/lib/dd/nats` via `hostPath`, which fits
the current single-node EC2 cluster. Move this to a real storage class before
turning the cluster into a multi-node setup.

The runtime queue path uses JetStream stream `DD_REMOTE_TASKS` for
`dd.remote.thread.*.tasks`. `dd-remote-queue-consumer` binds durable pull
consumer `dd-remote-thread-preparer`, and KEDA reads the NATS monitoring endpoint
on `:8222` to scale that deployment by consumer lag.
