# `remote/argocd/messaging`

GitOps-managed messaging layer for remote-dev.

## Components

- `dd-nats`: NATS server with JetStream enabled.
- `prometheus-exporter` sidecar: `natsio/prometheus-nats-exporter`, exposing
  NATS metrics on `:7777`.
- `dd-nats-bridge`: cluster-internal HTTP-to-JetStream ingress for external
  producers that must not receive raw NATS access.

## In-cluster endpoints

- NATS client URL: `nats://dd-nats.messaging.svc.cluster.local:4222`
- NATS monitoring: `http://dd-nats.messaging.svc.cluster.local:8222`
- Prometheus metrics: `http://dd-nats.messaging.svc.cluster.local:7777/metrics`
- Queue ingress service: `http://dd-nats-bridge.messaging.svc.cluster.local:3004`

Data is stored on the EC2 host at `/var/lib/dd/nats` via `hostPath`, which fits
the current single-node EC2 cluster. Move this to a real storage class before
turning the cluster into a multi-node setup.

## Security posture and remaining hardening

The NATS server still has **no application-layer authentication or subject
authorization**: there is no accounts/users/token/nkey/JWT/TLS authorization
block. `nats.networkpolicy.yaml` now limits which namespaces may reach the
client, monitoring, and exporter ports, but any compromised or misconfigured
pod inside an allowed client namespace can still publish to or subscribe from
any subject permitted by the unauthenticated server.

This is the trust boundary the settlement system relies on: the
`dd.remote.contracts.solana.{settle,resolve}` subjects are on-chain broadcast
triggers, and all `*.results`/events are readable to allowed NATS clients.
NATS-initiated broadcast remains **off by default**
(`CONTRACT_NATS_SETTLEMENT_ENABLED=false`), so settle/resolve messages only
validate and simulate. Mainnet broadcast remains double-gated by
`SOLANA_MAINNET_SETTLEMENT_ENABLED`, and `dd-contract-service` refuses to enable
NATS broadcast without
`CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS=true`.

Before enabling any NATS-initiated broadcast, configure NATS accounts/nkey auth
with per-subject publish/subscribe permissions. Restrict settle/resolve to the
legitimate publisher and `dd-contract-service` subscriber, and update the
NetworkPolicy from namespace selectors to the smallest practical workload set.
This is a cluster-wide migration: inventory every producer and consumer, issue
least-privilege credentials, roll clients deliberately, and remove the explicit
unauthenticated-bus acknowledgement only after verification.

The runtime queue path uses JetStream stream `DD_REMOTE_TASKS` for
`dd.remote.thread.*.tasks`. `dd-remote-queue-consumer` binds durable pull
consumer `dd-remote-thread-preparer`, and KEDA reads the NATS monitoring endpoint
on `:8222` to scale that deployment by consumer lag.

## Hardening delivered so far (2026-07-31)

- `nats.networkpolicy.yaml`: `:4222` ingress is scoped to known client
  namespaces (`default`, `shared-auth`, `voxletra`, `messaging`, `ai-ml`, and
  `daedalus`), `:8222` to `keda` plus `observability`, and `:7777` to
  `observability`. This is the network half of the remaining account/nkey
  rollout.
- `dd-nats-bridge`: external callers use named
  `POST /v1/queues/:route` endpoints rather than raw subjects. Client-scoped
  bearer credentials grant explicit routes; each route maps internally to one
  exact subject and expected JetStream stream. Every request requires a bounded
  idempotency key, which is namespaced and sent as `Nats-Msg-Id`; the bridge also
  sends `Nats-Expected-Stream` and waits for the JetStream acknowledgement.
  There is no core-NATS fallback. JSON-object bodies have per-route limits,
  publish concurrency is capped at 64, publish timeouts are bounded, and public
  errors are stable and redacted. Routes come from
  `dd-nats-bridge-routes`; client grants come from the
  `BRIDGE_CLIENTS_JSON` field of `dd/messaging/nats-bridge-secrets`.
- `nats-bridge.ingress.template.yaml` is deliberately inert and absent from the
  Kustomization. Do not activate it until the TLS, hostname, secret,
  NetworkPolicy, rate-limit, negative-test, and log-redaction gates in
  `docs/nats-external-http-ingress.md` are satisfied.
- `dd-remote-queue-consumer`: DLQ transfer is transactional in order—retry and
  durably acknowledge the idempotent DLQ record first, then `Term` the source
  message. Exhausted DLQ writes preserve the source message and expose dedicated
  Prometheus counters instead of dropping work. Invalid JSON and unsafe
  identifiers are preserved through the same DLQ path.

## Vapi work queue and autoscaling

JetStream stream `DD_VAPI_TASKS` (`dd.vapi.tasks.>`, work-queue retention) is
provisioned by `dd-rust-vapi-phone` at startup (`VAPI_NATS_URL`), which binds
durable pull consumer `dd-vapi-phone-worker`. External producers use the
`vapi-task` bridge route, currently mapped to `dd.vapi.tasks.external` with
expected stream `DD_VAPI_TASKS`; they never choose the subject or stream.
In-cluster producers remain subject to the NATS security posture above. KEDA
(`dd-rust-vapi-phone.scaledobject.yaml` in `dd-next-runtime`) scales the vapi
deployment from 1 to 6 replicas by consumer lag. Task shapes are documented in
`remote/deployments/rust-vapi-phone-rs/src/nats_worker.rs`.
