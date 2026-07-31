<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/3fa-backend` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/3FA-app/3fa-backend.rs` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/3fa-backend`.
<!-- END k8s-cluster-submodule-notice -->

# 3FA — Sync Server (backend)

Zero-knowledge sync for the 3FA authenticator. The server stores only an opaque,
client-encrypted vault blob plus a version vector — it can never read your OTP
seeds or your password. Written in Rust (axum + SeaORM/Postgres).

> One of three repos:
> - **`3fa-desktop.rs`** — desktop app (Rust + Slint)
> - **`3fa-backend.rs`** — this server (Rust + axum)
> - **`3fa-website`** — marketing/download site (Astro)
>
> The wire-protocol types live in [`src/protocol.rs`](src/protocol.rs), a copy
> kept byte-for-byte in sync with the frontend's copy (guarded by
> `PROTOCOL_VERSION`).

## Endpoints

| Method | Path                  | Auth | Purpose                              |
|--------|-----------------------|------|--------------------------------------|
| POST   | `/v1/auth/shared`     | JWT  | Enroll a device via a shared-auth access token, returns sync token |
| POST   | `/v1/auth/supabase`   | JWT  | Compatibility exchange of a Supabase token through shared-auth |
| GET    | `/v1/devices`         | ✓    | List this account's devices (id, name, created, last-seen, revoked) |
| GET    | `/v1/vault`           | ✓    | Pull the sealed vault blob           |
| POST   | `/v1/vault`           | ✓    | Push a sealed vault blob (version-vector reconciled) |
| POST   | `/v1/devices/revoke`  | ✓    | Revoke a device's sync token         |
| GET    | `/healthz`            | —    | Liveness                             |
| GET    | `/readyz`             | —    | Postgres readiness                   |
| GET    | `/metrics`            | —    | Prometheus metrics — **separate listener**, see below |

`/metrics` is not served on the API port. It has its own listener bound from
`METRICS_BIND_ADDR` (default `0.0.0.0:9091`), so whether telemetry is readable
is a property of this service and of the NetworkPolicy, not of an Ingress path
rule in another repository. Everything else is on `BIND_ADDR`.

"Auth ✓" is a service-local sync token (`Authorization: Bearer <sync_token>`).
"Auth JWT" is a short-lived shared-auth access token. The compatibility route
accepts a Supabase provider token but sends it to shared-auth for verification
and exchange; this service never verifies human login credentials itself.

## Security model

- **Zero-knowledge vault.** Clients E2E-encrypt the whole vault before upload
  (`protocol::SealedBlob`); the DB stores ciphertext only.
- **Account auth.** [shared-auth](https://github.com/shared-auth) owns human
  registration, login, provider exchange, token signing, and revocation.
  `/v1/auth/shared` introspects its access token; `/v1/auth/supabase` delegates
  the provider-token exchange to the same authority for older clients. A verified
  stable shared user id is mapped to the local zero-knowledge vault, then this
  service issues a separate per-device sync token (only its SHA-256 is stored).
  The retired local password endpoints are not mounted.
- **Device lifecycle.** Live devices per account are capped
  (`MAX_DEVICES_PER_ACCOUNT`), each authenticated request stamps `last_seen_at`,
  and `GET /v1/devices` lets an owner audit and revoke enrollments.
- **Sync.** Per-device version vectors give last-writer-wins-with-merge; a stale
  push gets a `Conflict` and must pull/merge/retry. A push is accepted only if
  its base vector is *causally reachable* — a device may advance only its own
  counter, never fabricate a sibling's — so one device cannot wedge another.
- **Telemetry boundary.** Ciphertext, passwords, bearer tokens, auth hashes, and
  device secrets must never be placed in logs, spans, metric labels, or message
  payloads. The server intentionally does not publish vault operations to the
  shared NATS bus: Postgres is the authoritative sync boundary. Any future NATS
  integration must use a generated subject contract and emit only compact,
  redacted lifecycle identifiers after the database transaction commits.

## Run locally

```bash
export DATABASE_URL=postgres://user:pass@localhost/threefa
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f migrations/0001_init.sql
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f migrations/0002_isolate_threefa_schema.sql
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f migrations/0003_supabase_auth.sql
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f migrations/0004_shared_auth_identity.sql
cargo run
# API on :8080 (override with BIND_ADDR)
# metrics on :9091 (override with METRICS_BIND_ADDR)
```

Database changes are an explicit operator step: the server never applies DDL on
startup. The ORES cluster's declarative pg-defs contract at
`remote/libs/pg-defs/schema/schema.sql` is production's source of truth and is
applied only through a reviewed declarative migration. The frozen SQL files in
this repo remain useful for local bootstrap and upgrading older standalone
installs; `0002_isolate_threefa_schema.sql` moves legacy public tables into the
service-owned `threefa` schema.

SeaORM entities compile without a live database. A database is needed only to
run the service or database-backed integration tests.

The dependency lock and SeaORM 2 require Rust 1.95 or newer. The repository
toolchain and deployment builder are both pinned to Rust 1.95.

## Observability

- JSON `tracing` records go to stdout for Promtail/Loki collection.
- HTTP spans preserve W3C `traceparent` and export over OTLP/HTTP. Configure
  `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`.
- JSON records include the active OTEL `trace_id` and `span_id`, allowing direct
  correlation from Loki logs to distributed traces.
- The default-deny NetworkPolicy permits OTLP only to TCP `4318` in the
  `observability` namespace; no general Internet egress is opened.
- `/metrics` exposes bounded route/status counters, request latency histograms,
  in-flight requests, SeaORM query count/latency, and vault-conflict counts for
  Prometheus. SQL statements and user identifiers are never metric labels. It is
  served on its own listener (`METRICS_BIND_ADDR`, default `0.0.0.0:9091`), which
  the NetworkPolicy opens to the `observability` namespace alone; ingress-nginx
  reaches `8080` only.
- Readiness runs its database probe as an ordinary instrumented query, so the
  most frequent database operation in a deployment is visible in
  `threefa_database_queries_total` and its latency histogram.
## Shared-auth identity

Set `SHARED_AUTH_BASE_URL` to the shared-auth service or gateway mount. If it is
unset, both human-identity enrollment routes return `501`; vault/device routes
continue to accept already-issued service-local sync tokens. Production uses
`http://dd-remote-gateway.default.svc.cluster.local/shared-auth`, a bounded
in-cluster hop allowed by the service NetworkPolicy.

New clients call `POST /v1/auth/shared` with a shared-auth access token and a
`{"device_name":…}` body. Older clients can call `/v1/auth/supabase`; the token
is exchanged at shared-auth first. In both cases the returned sync token—not a
human-login token—is used for `/v1/vault` and `/v1/devices`.

## Layout

```
src/main.rs        Minimal binary entrypoint
src/server.rs      API + metrics listeners, one graceful SIGTERM shutdown
src/config.rs      Environment configuration
src/app.rs         HTTP router and middleware composition
src/accounts.rs    Identity-enrollment response and device-name contracts
src/auth.rs        Service-local device sync tokens (auth runs before body parsing)
src/json.rs        Request-body JSON extractor with coarse, contract-shaped errors
src/devices.rs     Device handlers and persistence
src/vault_blob.rs  Sealed-blob handlers and reconciliation
src/entity.rs      SeaORM models for the `threefa` schema
src/shared_auth.rs Central shared-auth exchange/introspection client
src/supabase_auth.rs Shared/provider enrollment and account mapping
src/telemetry.rs   OTEL traces and Loki-compatible JSON logs
src/metrics.rs     Prometheus HTTP, database, and domain metrics
src/protocol.rs    Wire-protocol DTOs (duplicated with the frontend)
migrations/        Frozen local/legacy bootstrap SQL
deploy/            Dockerfile + Kubernetes/Argo CD manifests
```

## Deploy

The canonical repo is already registered in ORES `k8s-cluster` as the secondary
submodule `remote/deployments/3fa-backend`. Develop and validate here, push the
canonical commit, then bump only that submodule pointer in the cluster repo.
Argo CD tracks the private upstream repo directly because the cluster repo-server
currently has recursive submodule checkout disabled. See
[`deploy/README.md`](deploy/README.md) for the exact boundary.

## License

MIT OR Apache-2.0

> **ORM policy:** application persistence uses **SeaORM**. Its `sqlx-postgres`
> feature is the database driver beneath SeaORM; do not add direct `sqlx` calls
> or a direct `sqlx` dependency.
>
> The SeaORM 2 upgrade is intentionally PostgreSQL-only. SQLx 0.9 makes MySQL
> RSA support explicit opt-in, so the unmaintained `rsa` crate is absent from
> `Cargo.lock` and `sqlx-mysql` is inactive. SeaORM 2 requires statement
> builders by reference, but no schema migration or application query rewrite
> was required. The only operational compatibility change is the coordinated
> Rust MSRV/toolchain increase from 1.88 to 1.95.

> **Locking/leases:** if this service ever needs distributed locks or leases,
> use the fiducia-cloud primitives (github.com/fiducia-cloud) rather than
> rolling our own.
