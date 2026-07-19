# daedalus-api-server

JSON API for [daedalus-fab](https://github.com/daedalus-fab) — fabrication plans,
imported designs, released instructions, and runs. Part of the **MASH** stack
(Maud, Axum, Supabase, SeaORM, htmx); this is the **A/S** tier (Axum + SeaORM
JSON API). The [daedalus-web-server](https://github.com/daedalus-fab) renders the
Maud/htmx UI over the same data; customers automate against *this* server via the
[daedalus-clients](https://github.com/daedalus-fab) SDKs.

## Data & namespace

Domain data lives in the **`daedalus` Postgres schema** on the shared pg-defs RDS
database (the fiducia/benefactor named-schema pattern). Entities come from the
generated `dd-pg-defs-sea-orm` adapter and are schema-qualified. The schema
contract is `remote/libs/pg-defs/schema/schema.sql`; migrate with
[`scripts/dpm.sh`](scripts/dpm.sh), never at boot.

Client-streamed telemetry is a **separate** system: clients write it directly to
Supabase (`public.daedalus_client_log_*`) over Realtime, with no hop through this
server.

## Auth

Requests to `/v1/*` present a **Supabase bearer token**. The server verifies the
JWT (HS256 secret or asymmetric JWKS, `aud`/`iss` pinned, key rotation cached and
rate-limited) and then enforces an **email allow-list** — cryptographic validity
is necessary but not sufficient. Auth **fails closed**: without both a key and a
non-empty allow-list, `/v1/*` returns `503`.

The **service-role key is never used here.** It bypasses RLS and belongs to
offline operator tooling only.

## Endpoints

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/health` | no | liveness |
| GET | `/ready` | no | readiness (database reachable) |
| GET | `/metrics` | no | Prometheus text exposition |
| GET | `/v1/plans` | yes | list the caller's plans |
| POST | `/v1/plans` | yes | create a plan |
| GET | `/v1/plans/:id` | yes | fetch one plan (404 if not the caller's) |
| GET | `/v1/plans/:id/events` | yes | websocket: live plan/run events |

## Configuration

| Env | Default | Meaning |
|-----|---------|---------|
| `HOST` / `PORT` | `0.0.0.0` / `8114` | bind address |
| `DAEDALUS_API_DATABASE_URL` | — | Postgres DSN (falls back to `DATABASE_URL`, `RDS_DATABASE_URL`) |
| `DAEDALUS_API_DATABASE_REQUIRED` | `false` | fail boot if no DSN |
| `DAEDALUS_API_SUPABASE_JWT_SECRET` | — | HS256 secret (legacy Supabase tokens) |
| `DAEDALUS_API_SUPABASE_JWKS_URL` | — | JWKS endpoint (asymmetric tokens) |
| `DAEDALUS_API_SUPABASE_AUDIENCE` | `authenticated` | pinned `aud` claim |
| `DAEDALUS_API_SUPABASE_ISSUER` | — | pinned `iss` claim (optional) |
| `DAEDALUS_API_ALLOWED_EMAILS` | — | comma-separated allow-list (**required** for `/v1`) |

## Build & test

Path deps resolve only inside the `k8s-cluster` superproject at
`remote/deployments/daedalus-api-server-rs`. From there:

```sh
cargo test        # unit tests: auth gate, validation, error redaction, ws routing
cargo clippy --all-targets
cargo fmt --check
```

Standalone CI (this repo) runs hygiene + `cargo fmt --check` only, by design — see
[AGENTS.md](AGENTS.md).
