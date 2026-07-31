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

The server currently runs with **no authentication/authorization** (no
`authorization` block, accounts, users, tokens, nkey/jwt, or TLS) and there is
**no NetworkPolicy** in this namespace, so any pod that can reach
`dd-nats.messaging.svc.cluster.local:4222` can publish to or subscribe from any
subject. This is the trust boundary the settlement system relies on: the
`dd.remote.contracts.solana.{settle,resolve}` subjects are on-chain broadcast
triggers, and all `*.results`/events are readable cluster-wide.

This is tolerated today only because:

- NATS-initiated broadcast is **off by default** (`CONTRACT_NATS_SETTLEMENT_ENABLED=false`),
  so settle/resolve messages only validate + simulate; and
- mainnet broadcast stays double-gated (`SOLANA_MAINNET_SETTLEMENT_ENABLED`), and
  `dd-contract-service` refuses to enable NATS broadcast without
  `CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS=true`.

**Before enabling any NATS-initiated broadcast**, lock the bus down: configure
NATS accounts/nkey auth with per-subject publish/subscribe permissions (restrict
the settle/resolve subjects to the legitimate publisher and `dd-contract-service`
subscriber), and add a messaging-namespace NetworkPolicy allowing `:4222` ingress
only from the known client set. Both changes are cluster-wide (every NATS client
needs credentials/allow-listing), so they must be rolled out deliberately with the
full pub/sub inventory — not piecemeal.

The runtime queue path uses JetStream stream `DD_REMOTE_TASKS` for
`dd.remote.thread.*.tasks`. `dd-remote-queue-consumer` binds durable pull
consumer `dd-remote-thread-preparer`, and KEDA reads the NATS monitoring endpoint
on `:8222` to scale that deployment by consumer lag.

## Hardening delivered so far (2026-07-24)

- `nats.networkpolicy.yaml`: `:4222` ingress is now scoped to the known client
  namespaces (`default`, `shared-auth`, `voxletra`, `messaging`, `ai-ml`,
  `daedalus`), `:8222` to `keda` + `observability`, `:7777` to `observability`.
  This is the network half of the backlog above; account/nkey auth remains the
  deliberate rollout.
- `dd-nats-bridge` (this namespace, from `remote/nats-bridge`): hardened
  HTTP→NATS chokepoint for callers that shouldn't get raw bus access
  (e.g. the voxletra tenant). Bearer-token auth (fail-closed), subject
  allowlist (`dd.vapi.tasks.`, `vxl.` — never `$SYS`/`$JS`/wildcards), JSON-only
  bodies capped at 256KB, bounded in-flight publish pressure, and optional
  `Nats-Msg-Id` de-duplication. `dd.vapi.tasks.>` is durable-only: if its
  JetStream stream is absent, the bridge returns 503 instead of silently
  downgrading to core NATS. Token comes from ClusterSecretStore key
  `dd/messaging/nats-bridge-secrets`.
- `dd-remote-queue-consumer`: DLQ transfer is now transactional in order—
  retry and durably ACK the idempotent DLQ record first, then `Term` the
  source message. Exhausted DLQ writes preserve the source message and
  expose dedicated Prometheus counters instead of dropping work. Invalid
  JSON and unsafe identifiers are preserved in the same DLQ path.

## Vapi work queue + autoscaling

JetStream stream `DD_VAPI_TASKS` (`dd.vapi.tasks.>`, work-queue retention) is
provisioned by `dd-rust-vapi-phone` at startup (`VAPI_NATS_URL`), which binds
durable pull consumer `dd-vapi-phone-worker`. Producers publish through
`dd-nats-bridge`; KEDA (`dd-rust-vapi-phone.scaledobject.yaml` in
dd-next-runtime) scales the vapi deployment 1→6 replicas by consumer lag. Task
shapes are documented in `remote/deployments/rust-vapi-phone-rs/src/nats_worker.rs`.
