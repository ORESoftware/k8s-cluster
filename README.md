# shared-auth-nats-bridge.rs

The event plane between shared-auth HTTP services and cluster NATS
(`dd-nats.messaging.svc.cluster.local:4222`).

- **HTTP→NATS** — internal callers (shared-auth-server, the shared-auth-sync
  outbox flusher: *"publish eligible Postgres outbox changes to the provider"*)
  POST `{subject, payload}` to `/publish`; the bridge lands it broker-confirmed
  (`publish` + `flush`) on a `shared-auth.*` subject.
- **NATS→HTTP** — routes in `BRIDGE_DELIVERIES` fan subscribed subjects out to
  webhooks (with the concrete subject in `x-bridge-subject`), so services
  without a NATS client still react — session revocation, identity updates.

## Boundaries

- `POST /publish` is bearer-gated (`BRIDGE_INTERNAL_TOKEN`, env-only) and
  **prefix-constrained** (`shared-auth.` by default): this is the shared-auth
  event plane, not a generic NATS proxy. No wildcards are publishable.
- Delivery subscriptions may use NATS wildcards, but only under the same prefix.
- Core-NATS **at-most-once**: a 202 means the broker received the bytes; failed
  webhook deliveries are counted (`shared_auth_bridge_deliveries_total`) and
  dropped. JetStream durable consumers are the upgrade path when the sync
  outbox needs guaranteed delivery.
- NATS being down never crash-loops the bridge: the HTTP surface serves,
  `/readyz` reports 503, and a background loop reconnects (the tandem principle
  applied to messaging).

## Endpoints

`POST /publish` · `GET /healthz` · `GET /readyz` · `GET /metrics` (port 8121).

## Config

`.cli-flags.toml` via flags-2-env: `BRIDGE_BIND_ADDR`, `BRIDGE_NATS_URL`,
`BRIDGE_SUBJECT_PREFIX`, `BRIDGE_DELIVERIES` (JSON `[{subject, webhook}]`).
`BRIDGE_INTERNAL_TOKEN` is environment-only (≥16 chars, required).

## Deploy

`deploy/k8s/` — namespace-scoped manifests for the `shared-auth` tenant
(RollingUpdate maxUnavailable 0, PDB via k8s-cluster, scrape annotations,
NetworkPolicy: ingress from shared-auth peers + observability; egress DNS +
messaging:4222 + shared-auth:8120). ArgoCD tracks this repo directly per the
k8s-cluster app-deploy contract; the submodule pin there is inventory only.
