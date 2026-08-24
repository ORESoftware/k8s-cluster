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

## Security posture (hardening backlog)

The server currently runs with **no NATS-native authentication/authorization**
(no `authorization` block, accounts, users, tokens, nkey/jwt, or TLS). A
default-deny `NetworkPolicy` limits the client port to the `default`, `ai-ml`,
and `daedalus` namespaces plus the EC2 VPC host-network range, and limits the
unauthenticated monitoring ports to observability and three named tooling pods.
That is meaningful network isolation, but it is not subject-level identity:
any allowed client can still publish to or subscribe from any subject. This is
the trust boundary the settlement system relies on: the
`dd.remote.contracts.solana.{settle,resolve}` subjects are on-chain broadcast
triggers, and all `*.results`/events are readable by allowed NATS clients.

This is tolerated today only because:

- NATS-initiated broadcast is **off by default** (`CONTRACT_NATS_SETTLEMENT_ENABLED=false`),
  so settle/resolve messages only validate + simulate; and
- mainnet broadcast stays double-gated (`SOLANA_MAINNET_SETTLEMENT_ENABLED`), and
  `dd-contract-service` refuses to enable NATS broadcast without
  `CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS=true`.

**Before enabling any NATS-initiated broadcast**, lock the bus down: configure
NATS accounts/nkey auth with per-subject publish/subscribe permissions (restrict
the settle/resolve subjects to the legitimate publisher and `dd-contract-service`
subscriber), then narrow the existing namespace/VPC NetworkPolicy to named
service identities wherever the runtime topology permits. Authentication is
cluster-wide (every NATS client needs credentials), so it must be rolled out
deliberately with the full pub/sub inventory and a tested credential rotation
path — not piecemeal.

The runtime queue path uses JetStream stream `DD_REMOTE_TASKS` for
`dd.remote.thread.*.tasks`. `dd-remote-queue-consumer` binds durable pull
consumer `dd-remote-thread-preparer`, and KEDA reads the NATS monitoring endpoint
on `:8222` to scale that deployment by consumer lag.
