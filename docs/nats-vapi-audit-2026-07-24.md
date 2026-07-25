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

## Automated test harnesses (committed, repeatable)

Two scripts run real binaries against a real broker — no mocks:

- `remote/nats-bridge/scripts/e2e.sh` (26 checks) — bridge + worker + NATS.
- `vxl-api-server.rs/scripts/e2e-nats-chain.sh` (in the voxletra repo, 20
  checks) — the full cross-repo chain: vxl-api-server → dd-nats-bridge →
  JetStream → dd-rust-vapi-phone, including competing consumers, tenant
  isolation, degraded modes, and a KEDA config-contract check that parses the
  ScaledObject YAML and verifies its stream/consumer/account actually exist on
  the live broker (catches config drift before Argo does).

## End-to-end verification (local, real NATS 2.11.17-alpine in docker, JetStream on)

Ran the actual bridge binary and the actual vapi server binary against a real
broker (same image as the cluster):

- Bridge rejection matrix: no token → 401, wrong token → 401, off-allowlist
  subject (`dd.remote.contracts.solana.settle`) → 403, `$JS.API.STREAM.DELETE.*`
  → 403, wildcard `>` → 403, non-JSON body → 400, body over cap → 413. All
  counted in `rejected_total`; nothing reached the bus.
- Allowed subject with no stream bound → 200 `durable:false` (core fallback).
  This surfaced a bug during testing: async-nats reports "no stream found for
  given subject" (not only "no responders"); classifier fixed + unit test added.
- Vapi worker provisioned `DD_VAPI_TASKS` + `dd-vapi-phone-worker` at startup.
  Malformed task (`type:"reboot-cluster"`) → dropped + acked. `outbound-call`
  without `VAPI_PHONE_NUMBER_ID` → permanent drop + ack. `setup-refresh` with
  no `VAPI_API_KEY` → NAK'd and redelivered exactly `max_deliver` (3) times,
  then delivery stopped with `num_ack_pending: 0` — poison messages cannot
  wedge the consumer and do not inflate the KEDA lag signal.
- Scale signal: with the worker stopped, 5 bridge-published tasks →
  `num_pending: 5` on the monitoring endpoint (above the ScaledObject's
  `activationLagThreshold: 2`); worker restart drained it to 0.

Test-found fixes also applied to voxletra/vxl-api-server.rs: axum 0.7 uses
`:id` captures, so the pre-existing `/vapi/call/{id}` route was unreachable —
fixed with a regression test, plus 6 integration tests covering
`/vapi/call/dispatch` (auth fail-closed, bad token, bridge-unconfigured 503,
invalid number rejected before the bridge is touched, and the full queued
publish with subject/token/payload asserted against a fake bridge).

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
