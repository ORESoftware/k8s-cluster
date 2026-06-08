# `dd-benefactor-marketing-rs`

Rust Axum backend for a B2B marketing agency platform. The service covers client workspaces,
contacts, packages, contracts, invoices, lead imports, enrichment/scraper handoffs, lead scoring,
campaigns, channel plans, A/B experiments, automation workflows, attribution events, reports,
opportunities, content assets, project tasks, approvals, tickets, and meetings.

## Database Contract

The tables live in `remote/libs/pg-defs/schema/schema.sql` under the
`benefactor_marketing_` prefix. ORM models are generated through:

```sh
cd remote/libs/pg-defs
node src/generate.mjs
```

The Rust service depends on `remote/libs/pg-defs/generated/rust/sea-orm`, so schema changes should
always update the SQL contract first and regenerate pg-defs. Do not derive migrations from Rust
models.

Use `BENEFACTOR_MARKETING_DATABASE_URL` for a dedicated marketing Postgres database. `DATABASE_URL`
is accepted as a local fallback only.

## Runtime

Required:

- `BENEFACTOR_MARKETING_DATABASE_URL`
- `BENEFACTOR_MARKETING_API_AUTH_BEARER`

Optional:

- `BENEFACTOR_MARKETING_HOST` (default `0.0.0.0`)
- `BENEFACTOR_MARKETING_PORT` (default `8134`)
- `BENEFACTOR_MARKETING_LOG_FORMAT=json`
- `BENEFACTOR_MARKETING_SCRAPER_BASE_URL`
- `BENEFACTOR_MARKETING_ALLOW_UNAUTHENTICATED=true` for local-only development

Web scraping is intentionally offloaded. `POST /leads/{lead_id}/enrichment-jobs` records the
handoff job and, when `BENEFACTOR_MARKETING_SCRAPER_BASE_URL` is set, stamps a deterministic
external handoff URL for the scraper service.

## Routes

Generated docs are served at:

- `GET /docs/api`
- `GET /api/docs`
- `GET /api/docs.json`

Operational routes:

- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /capabilities`

Core domain routes include:

- `GET|POST /clients`
- `GET /clients/{client_id}/dashboard`
- `POST /clients/{client_id}/contacts`
- `POST /leads/import`
- `POST /leads/{lead_id}/enrichment-jobs`
- `POST /leads/{lead_id}/score`
- `POST /campaigns`
- `POST /campaigns/{campaign_id}/channels`
- `POST /automation/workflows`
- `POST /reports/snapshots`
- `POST /attribution/events`
- `POST /opportunities`
- `POST /content/assets`
- `POST /projects/tasks`
- `POST /approvals`
- `PATCH /approvals/{approval_id}/decision`
- `POST /tickets`
- `POST /meetings`

Domain routes require either `Authorization: Bearer <token>` or the legacy `Auth` header matching
`BENEFACTOR_MARKETING_API_AUTH_BEARER`.
