# `remote/deployments/usacc-rest-api-backend-rs`

Rust/Axum REST backend for the US Anti-Corruption Court project.

## Purpose

- Exposes JSON-only REST routes for frontend CRUD applications.
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
