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
| POST   | `/v1/auth/supabase`   | JWT  | Enroll a device via a Supabase access token, returns sync token |
| GET    | `/v1/devices`         | ✓    | List this account's devices (id, name, created, last-seen, revoked) |
| GET    | `/v1/vault`           | ✓    | Pull the sealed vault blob           |
| POST   | `/v1/vault`           | ✓    | Push a sealed vault blob (version-vector reconciled) |
| POST   | `/v1/devices/revoke`  | ✓    | Revoke a device's sync token         |
| GET    | `/healthz`            | —    | Liveness                             |
| GET    | `/readyz`             | —    | Postgres readiness                   |
| GET    | `/metrics`            | —    | Prometheus metrics                   |

"Auth ✓" is an account sync token (`Authorization: Bearer <sync_token>`). "Auth
JWT" is a Supabase access token in the same header — the server verifies it and
issues a sync token in exchange.

## Security model

- **Zero-knowledge vault.** Clients E2E-encrypt the whole vault before upload
  (`protocol::SealedBlob`); the DB stores ciphertext only.
- **Account auth.** Two identity sources, both issuing the same per-device bearer
  sync token (only the token's SHA-256 is stored):
  - *Supabase (preferred).* Supabase Auth owns login (email/password, OAuth, MFA)
    and mints a short-lived access JWT. `/v1/auth/supabase` verifies that JWT —
    signature via the project JWKS (RS256/ES256, `kid`-selected, cached; legacy
    HS256 shared-secret supported) plus strict `exp`/`aud`/`iss` — and maps `sub`
    onto a local account. **The server never receives a password**, so login and
    the E2E vault key are fully separated.
  - *Legacy.* `/v1/register` + `/v1/login` with an Argon2id verifier; login is
    constant-work to avoid user enumeration. Being phased out in favor of Supabase.
- **Device lifecycle.** Live devices per account are capped
  (`MAX_DEVICES_PER_ACCOUNT`), each authenticated request stamps `last_seen_at`,
  and `GET /v1/devices` lets an owner audit and revoke enrollments.
- **Sync.** Per-device version vectors give last-writer-wins-with-merge; a stale
  push gets a `Conflict` and must pull/merge/retry. A push is accepted only if
  its base vector is *causally reachable* — a device may advance only its own
  counter, never fabricate a sibling's — so one device can't wedge another.
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

## Supabase identity

Set these to enable `/v1/auth/supabase` (unset ⇒ the route returns `501`, and the
server runs legacy-auth only):

- `SUPABASE_PROJECT_URL` — e.g. `https://<ref>.supabase.co`. The issuer
  (`<url>/auth/v1`) and JWKS URL (`<url>/auth/v1/.well-known/jwks.json`) are
  derived from it.
- `SUPABASE_JWT_AUD` — expected audience (default `authenticated`).
- `SUPABASE_JWT_LEGACY_SECRET` — only if the project still signs with the legacy
  HS256 shared secret. Prefer asymmetric signing keys (RS256/ES256) and leave this
  unset; the server resolves those from the JWKS automatically and needs no secret.

The client obtains the access JWT from Supabase, then calls `POST
/v1/auth/supabase` with `Authorization: Bearer <jwt>` and a `{"device_name":…}`
body to receive a long-lived sync token. The sync token — not the JWT — is used
for `/v1/vault` and `/v1/devices`, so an expired JWT does not force a full vault
re-auth; the client silently refreshes its Supabase session (unlocked locally by
the app's 6-digit PIN) and keeps its existing sync token.

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

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack). This
> service still uses direct sqlx — conversion to SeaORM is pending; see the
> fiducia-messaging.rs migration for the reference playbook.

> **Locking/leases:** if this service ever needs distributed locks or leases,
> use the fiducia-cloud primitives (github.com/fiducia-cloud) rather than
> rolling our own.
