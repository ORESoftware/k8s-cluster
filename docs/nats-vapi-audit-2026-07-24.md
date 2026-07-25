# NATS + Vapi interop audit and hardening — 2026-07-24

Scope: `remote/nats-bridge`, `remote/argocd/messaging`, the vapi phone server
(`remote/deployments/rust-vapi-phone-rs` + its dd-next-runtime manifests), and
the Voxletra tenant's path onto the bus (voxletra/vxl-api-server.rs).

## Findings

| # | Severity | Finding | Status |
|---|----------|---------|--------|
| 1 | critical | `remote/nats-bridge` was an unauthenticated open relay: any pod could POST any payload to **any** subject, including `$JS.API.>` (stream deletion) and the settlement subjects the messaging readme flags as on-chain triggers. | fixed (rewrite) |
| 2 | high | NATS server runs with no authentication and had no NetworkPolicy; any pod in any namespace could pub/sub all subjects. | partially fixed: NetworkPolicy added; account/nkey auth remains the documented deliberate rollout |
| 3 | medium | Bridge publishes were core-NATS fire-and-forget: a 200 response did not mean the message existed anywhere durable. | fixed (JetStream acked publish, `durable` flag in response) |
| 4 | medium | Bridge crashed on NATS unavailability (`expect` on connect), no readiness probe semantics, no body limits, no graceful shutdown. | fixed |
| 5 | medium | No NATS-driven autoscaling for vapi work: bursts of call tasks had nowhere to queue and nothing scaled the workers. | fixed (DD_VAPI_TASKS + KEDA) |
| 6 | low | vapi phone pod egress policy did not allow NATS (blocked the new worker). | fixed |
| 7 | low | Voxletra services would have needed raw bus access for interop. | fixed (bridge is the tenant chokepoint) |
| 8 | info | vapi webhook/setup auth was already fail-closed (`VAPI_SERVER_SECRET`, `SERVER_AUTH_SECRET` non-optional); vxl-api-server webhook likewise (constant-time compare, bounded audit rows). | no change needed |

## Changes in this repo (working tree, uncommitted)

- `remote/nats-bridge/src/main.rs` — rewritten: bearer/`x-bridge-token` auth
  (fail-closed startup, ≥16-char token), subject allowlist via
  `BRIDGE_SUBJECT_PREFIXES` (no `$`-subjects, wildcards, or out-of-list
  publishes), JSON-only bodies capped by `BRIDGE_MAX_BODY_BYTES` (256KB),
  JetStream acked publish with core-NATS fallback for non-stream subjects,
  retrying connect, `/healthz` + `/readyz`, graceful shutdown, counters,
  6 unit tests.
- `remote/argocd/messaging/` — new `nats.networkpolicy.yaml`,
  `nats-bridge.{deployment,service,networkpolicy,externalsecret}.yaml`
  (run-from-source convention), kustomization + readme updated.
- `remote/argocd/dd-next-runtime/` — new `dd-rust-vapi-phone.scaledobject.yaml`
  (KEDA nats-jetstream, stream `DD_VAPI_TASKS`, consumer
  `dd-vapi-phone-worker`, 1→6 replicas), vapi deployment gains
  `VAPI_NATS_URL`, vapi NetworkPolicy gains messaging:4222 egress,
  kustomization updated. Both kustomizations `kubectl kustomize`-clean.
- `remote/deployments/rust-vapi-phone-rs/` — new `src/nats_worker.rs`:
  JetStream pull consumer for `dd.vapi.tasks.>` (work-queue retention,
  provisioned at startup), task types `outbound-call` / `setup-refresh`,
  poison-message ack policy, transient-failure NAK with bounded redelivery;
  24 tests pass (5 new).

## Scale-up path ("bridge launches vapi pods")

Deliberately implemented via KEDA rather than the bridge creating Jobs or
Threads itself: the bridge publishes durable tasks; KEDA's `nats-jetstream`
scaler (already installed, same pattern as `dd-remote-queue-consumer`) watches
consumer lag on `:8222` and scales the `dd-rust-vapi-phone` deployment. The
bridge stays a dumb, auditable chokepoint with no k8s API credentials —
`automountServiceAccountToken: false` stays true. `dd-thread-operator` remains
the right tool for per-entity pods, not for queue-depth scaling.

## Rollout order

1. Seed ClusterSecretStore key `dd/messaging/nats-bridge-secrets`
   (`BRIDGE_TOKEN`, ≥16 random chars).
2. Commit/push these changes to `dev` (Argo syncs messaging + dd-next-runtime).
3. Verify: bridge `/readyz` OK; `nats stream info DD_VAPI_TASKS` exists after
   the vapi pod restarts; publish a `setup-refresh` task through the bridge and
   watch the worker log; queue >2 tasks and watch KEDA scale.
4. Voxletra side (already pushed): set `VXL_BRIDGE_URL` +
   `VXL_BRIDGE_TOKEN` on vxl-api-server, use `POST /vapi/call/dispatch`.

## Remaining backlog (unchanged in spirit from messaging readme)

- Full NATS account/nkey auth with per-subject permissions (needs the complete
  pub/sub inventory; every client changes at once).
- TLS on 4222 (or rely on a service mesh later).
- JetStream storage off hostPath before multi-node.
- The vapi deployment still builds from source at pod start (rust:bookworm +
  `cargo run`); prebuilt images would shrink startup and the scale-up latency
  KEDA can deliver.
