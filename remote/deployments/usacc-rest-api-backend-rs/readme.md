# `remote/deployments/usacc-rest-api-backend-rs`

Rust/Axum REST backend for the US Anti-Corruption Court project.

## Purpose

- Exposes JSON REST routes for frontend CRUD applications.
- Serves a server-rendered HTMX operator console at `/app` from the same
  Axum binary (see [Operator console](#operator-console)).
- Reads and writes the USACC Postgres contract tables owned by
  `remote/libs/pg-defs/schema/schema.sql`.
- Imports `remote/submodules/discrete-event-system.rs` as the `des_engine` Rust crate for
  deterministic simulation runs.
- Implements first-pass voting tally logic, accounting ledger summaries, and a Solana contract
  validation/simulation bridge to `dd-contract-service`.

The target frontend project lives at
`/Users/maca5/codes/ores/us-anti-corruption-court-project`; this backend is deployed from the
cluster repo because it owns RDS/Postgres, runtime service, and gateway wiring.

## Routes

The served route docs are generated from source by `remote/tools/generate-api-docs.mjs`; this
README is narrative context, not a route inventory. HTML docs are available at `/docs/api` and
`/api/docs`; JSON metadata is available at `/api/docs.json`.

Main API prefix: `/api/usacc`.

Core surfaces:

- Users: list, create, read, patch.
- Cases: list, create, read, patch.
- Stages: list/create per case.
- Elections and votes: create elections, cast votes, list votes, certify a tally.
- Accounting: create ledger entries and read case ledger summaries.
- Contracts: validate/simulate envelopes through the Rust Solana contract service.
- Simulations: run a deterministic DES-backed court simulation and optionally persist it.

## Operator console

A server-rendered HTMX UI mounted at `/app`, a parallel surface to the JSON
API over the same Postgres pool. It uses [maud](https://maud.lambda.xyz) for
templating and a vendored, SRI-pinned HTMX served from `/app/static/` (no
CDN, strict `script-src 'self'` CSP). Surfaces:

- Dashboard — record counts and a live (5s) DB status pill.
- Cases — file cases and drill into stages, elections, and ledger totals.
- Users — register participants and entities.
- Elections — open ballots, review votes, and run a tally that certifies
  the election (same arithmetic as `POST /api/usacc/elections/:id/tally`).
- Simulations — run the deterministic DES court simulation and optionally
  persist the run.

Security mirrors the JSON API's posture: a CSRF guard requires `HX-Request`
(or a same-site `Sec-Fetch-Site`) on every write, an optional
`USACC_APP_UI_BEARER` gates all console requests, and strict security
headers (CSP, anti-clickjacking, `noindex`) are set on every response.

Every link, form action, and HTMX target is built with `USACC_APP_BASE_PATH`
so the same binary works both directly (`/app`, base empty) and behind the
path-stripping gateway (`/usacc/app`, base `/usacc`).

## Environment

Database lookup order:

1. `USACC_DATABASE_URL`
2. `RDS_DATABASE_URL`
3. `DATABASE_URL`
4. `AGENT_TASKS_RDS_DATABASE_URL`

Other important variables:

- `HOST` / `PORT` (`PORT` defaults to `8121`)
- `SERVER_AUTH_SECRET` or `USACC_API_AUTH_SECRET`
- `USACC_API_AUTH_REQUIRED` (defaults to `true`)
- `USACC_CONTRACT_SERVICE_URL` (defaults to
  `http://dd-contract-service.default.svc.cluster.local:8101`)
- `USACC_APP_UI_ENABLED` (defaults to `true`) — mount the `/app` console.
- `USACC_APP_BASE_PATH` (defaults to empty) — external path prefix the
  console is reached through (set to `/usacc` behind the gateway).
- `USACC_APP_UI_BEARER` — optional `Authorization: Bearer` token gating the
  console. Unset means open (front it with the gateway auth).
- `USACC_APP_UI_ALLOWED_ORIGINS` — comma-separated extra origins permitted
  to issue console writes (the request `Host` is always same-origin).

If no database URL is configured the service still boots and reports that state from `/healthz`;
database-backed API routes return `503`.

## Deployment

Argo includes this service as `usacc-rest-api-backend-rs` in
`remote/argocd/dd-next-runtime`. The in-cluster service listens on
`usacc-rest-api-backend-rs.default.svc.cluster.local:8121`; the public gateway exposes the
frontend-facing API at `/api/usacc/*` and operator docs/health at `/usacc/*`.

## Postgres Contract

The USACC tables use the `usacc_` table namespace in `pg-defs`, while the live connection can point
at the dedicated `usacc` database through `USACC_DATABASE_URL`.

Do not generate SQL from the Rust route code. Schema changes go through
`remote/libs/pg-defs/schema/schema.sql`, followed by `node remote/libs/pg-defs/src/generate.mjs`.
