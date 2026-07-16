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
seeds or your password. Written in Rust (axum + sqlx/Postgres).

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
| POST   | `/v1/register`        | —    | Create account + first device, returns token |
| POST   | `/v1/login`           | —    | Verify account, register a device, returns token |
| GET    | `/v1/vault`           | ✓    | Pull the sealed vault blob           |
| POST   | `/v1/vault`           | ✓    | Push a sealed vault blob (version-vector reconciled) |
| POST   | `/v1/devices/revoke`  | ✓    | Revoke a device's sync token         |
| GET    | `/healthz`            | —    | Liveness                             |
| GET    | `/readyz`             | —    | Postgres readiness                   |
| GET    | `/metrics`            | —    | Prometheus metrics                   |

## Security model

- **Zero-knowledge vault.** Clients E2E-encrypt the whole vault before upload
  (`protocol::SealedBlob`); the DB stores ciphertext only.
- **Account auth.** Argon2id verifier + per-device bearer tokens (only the
  token's SHA-256 is stored; login is constant-work to avoid user enumeration).
  OPAQUE PAKE is the planned drop-in (`src/auth.rs`) so the password never
  reaches the server at all.
- **Sync.** Per-device version vectors give last-writer-wins-with-merge; a stale
  push gets a `Conflict` and must pull/merge/retry.
- **Telemetry boundary.** Ciphertext, passwords, bearer tokens, auth hashes, and
  device secrets must never be placed in logs, spans, metric labels, or message
  payloads. The server intentionally does not publish vault operations to the
  shared NATS bus: Postgres is the authoritative sync boundary. Any future NATS
  integration must use a generated subject contract and emit only compact,
  redacted lifecycle identifiers after the database transaction commits.

## Run locally

```bash
DATABASE_URL=postgres://user:pass@localhost/threefa sqlx migrate run
DATABASE_URL=postgres://user:pass@localhost/threefa cargo run
# serves on :8080 (override with BIND_ADDR)
```

Migrations are an explicit operator step: the server never applies DDL on
startup. Review the SQL before running `sqlx migrate run`, and use the shared
declarative Postgres contract when deploying into the ORES cluster. Migration
`0002_isolate_threefa_schema.sql` moves the legacy public tables into the
service-owned `threefa` schema; confirm that the source tables belong to 3FA
before applying it to a shared database.

`sqlx` uses runtime (non-macro) queries, so **no live database is needed to
build** — only to run.

The dependency lock currently requires Rust 1.88 or newer. The deployment
builder is pinned to the multi-architecture Rust 1.95 Bookworm image digest.

## Observability

- JSON `tracing` records go to stdout for Promtail/Loki collection.
- HTTP spans preserve W3C `traceparent` and export over OTLP/HTTP. Configure
  `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`.
- The default-deny NetworkPolicy permits OTLP only to TCP `4318` in the
  `observability` namespace; no general Internet egress is opened.
- `/metrics` exposes bounded route/status counters, request latency histograms,
  and vault-conflict counts for Prometheus.
- `THREEFA_AUTH_MAX_CONCURRENT` (default `2`) bounds concurrent Argon2 work;
  excess login/register requests fail with `429` instead of exhausting memory.

## Layout

```
src/app.rs        Router, handlers, app state
src/auth.rs       Argon2id verifier + bearer tokens (OPAQUE seam)
src/vault_blob.rs Sealed-blob store + version-vector reconciliation
src/devices.rs    Device registration / revocation
src/db.rs         Bounded Postgres pool (DDL stays operator-owned)
src/protocol.rs   Wire-protocol DTOs (duplicated with the frontend)
migrations/       sqlx Postgres migrations
deploy/           Dockerfile + k8s/ArgoCD manifests
```

## Deploy

To the ORES `k8s-cluster` as a git submodule — see
[`deploy/README.md`](deploy/README.md).

## License

MIT OR Apache-2.0
