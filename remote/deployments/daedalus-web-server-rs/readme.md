# daedalus-web-server

Server-rendered UI for [daedalus-fab](https://github.com/daedalus-fab) —
fabrication plans and their runs, as HTML. This is the **M/H** tier of the
**MASH** stack (Maud + htmx); the [daedalus-api-server](https://github.com/daedalus-fab)
is the JSON/write tier over the same data.

**Read-only.** This process never writes the `daedalus` schema — every mutation
goes through the API server. It renders Maud HTML and pushes live updates over a
websocket using the htmx-ws pattern.

## How the live view works

A plan page opens a websocket (`/plans/:id/runs/ws`). The server polls the
`daedalus` schema and pushes a replacement `#runs` HTML fragment whenever the
plan's runs change — the browser needs no JSON-handling JS, htmx swaps the
fragment by id. The same `views::runs_fragment` renders the initial paint and
every push, so there is one definition of a run row.

This is the server-push half of the org's websocket story. Clients that want
low-latency telemetry subscribe to **Supabase Realtime** directly; this server
polls Postgres because it is a separate deployment from the writer and has no
shared in-memory bus.

## Auth & data

Identical boundary to the API server: a **Supabase bearer token** whose `email`
claim is on `DAEDALUS_WEB_ALLOWED_EMAILS`. Verification alone is insufficient;
auth **fails closed** (503 on `/`) when either the key or the allow-list is
missing. Ownership is enforced by filtering every query on the verified email —
this database has **no RLS**. A plan that isn't yours returns **404**, never 403.

Data lives in the `daedalus` Postgres schema (shared pg-defs RDS), read through
the generated `dd-pg-defs-sea-orm` adapter.

## Assets & CSP

htmx and its websocket extension are **vendored and served from `/assets`** on
this origin, not a CDN, so the response `Content-Security-Policy` can stay
`default-src 'self'`. The version is pinned in `src/views.rs` (`HTMX_VERSION`)
and coupled to the asset filenames by a test.

> ⚠️ `assets/htmx.min.js` and `assets/htmx-ws.min.js` are **placeholders**.
> Replace them with the real pinned htmx 1.9.12 builds before deploying — the
> curl commands are in each file's header comment. The crate compiles and tests
> pass with the placeholders; the UI will not function until they are replaced.

## Endpoints

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/health` / `/ready` / `/metrics` | no | ops |
| GET | `/assets/:name` | no | pinned htmx bundles |
| GET | `/` | yes | the caller's plans |
| GET | `/plans/:id` | yes | one plan + runs |
| GET | `/plans/:id/runs` | yes | runs fragment (htmx) |
| GET | `/plans/:id/runs/ws` | yes | live runs fragment (websocket) |

## Configuration

`HOST`/`PORT` (default `0.0.0.0:8115`); `DAEDALUS_WEB_DATABASE_URL`
(→ `DATABASE_URL` → `RDS_DATABASE_URL`); `DAEDALUS_WEB_SUPABASE_JWT_SECRET` /
`_JWKS_URL` / `_AUDIENCE` / `_ISSUER`; `DAEDALUS_WEB_ALLOWED_EMAILS` (required
for authenticated pages).

## Build & test

Path deps resolve only inside the `k8s-cluster` superproject at
`remote/deployments/daedalus-web-server-rs`. From there: `cargo test`,
`cargo clippy --all-targets`, `cargo fmt --check`. See [AGENTS.md](AGENTS.md).
