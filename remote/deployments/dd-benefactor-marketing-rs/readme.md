# `dd-benefactor-marketing-rs`

Rust Axum backend for a B2B marketing agency platform. The service covers client workspaces,
contacts, packages, contracts, invoices, lead imports, enrichment/scraper handoffs, lead scoring,
CRM sync runs, campaigns, channel plans, A/B experiments, outreach sequences, prospect research,
conversion tracking, automation workflows, attribution events, reports, opportunities, content
assets, project tasks, team allocations, approvals, tickets, and meetings.

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
- `BENEFACTOR_MARKETING_REDIS_URL` or `REDIS_URL` (default EC2 manifest points at
  `redis://dd-redis-cache.default.svc.cluster.local:6379/0`)
- `BENEFACTOR_MARKETING_REDIS_REQUIRED_FOR_READY=true` when Redis must participate in readiness
- `BENEFACTOR_MARKETING_CACHE_TTL_SECONDS` (default `120`; `0` disables cache writes)
- `BENEFACTOR_MARKETING_RATE_LIMIT_PER_MINUTE` (default `600`; `0` disables Redis throttling)
- `BENEFACTOR_MARKETING_JOB_STREAM` (default `benefactor:marketing:jobs`)
- `BENEFACTOR_MARKETING_ALLOW_UNAUTHENTICATED=true` for local-only development

Web scraping is intentionally offloaded. `POST /leads/{lead_id}/enrichment-jobs` records the
handoff job and, when `BENEFACTOR_MARKETING_SCRAPER_BASE_URL` is set, stamps a deterministic
external handoff URL for the scraper service. When Redis is configured, lead imports, enrichment
handoffs, CRM sync runs, outreach events, prospect research briefs, conversion events, automation
events, report snapshots, and attribution events are also published to the configured Redis stream
for workers or ETL services.

Redis is used for:

- client dashboard cache entries under `benefactor:marketing:client-dashboard:*`
- per-actor write throttling under `benefactor:marketing:rate:*`
- worker handoff events on `benefactor:marketing:jobs`

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
- `GET /runtime/redis`

Core domain routes include:

- `GET|POST /clients`
- `GET /clients/{client_id}/dashboard`
- `GET /clients/{client_id}/lead-intelligence`
- `GET /clients/{client_id}/revenue-attribution`
- `GET /clients/{client_id}/operations`
- `GET|POST /clients/{client_id}/team-allocations`
- `GET /clients/{client_id}/sync-runs`
- `GET /clients/{client_id}/outreach`
- `GET /clients/{client_id}/outreach/sequences`
- `GET /clients/{client_id}/research/briefs`
- `GET /clients/{client_id}/conversion-events`
- `POST /clients/{client_id}/contacts`
- `POST /integrations/{integration_id}/sync-runs`
- `POST /leads/import`
- `POST /leads/{lead_id}/enrichment-jobs`
- `POST /leads/{lead_id}/score`
- `POST /campaigns`
- `POST /campaigns/{campaign_id}/channels`
- `POST /campaigns/{campaign_id}/experiments`
- `POST /outreach/sequences`
- `POST /outreach/sequences/{sequence_id}/steps`
- `POST /outreach/enrollments`
- `POST /outreach/touchpoints`
- `POST /automation/workflows`
- `POST /automation/events`
- `POST /reports/snapshots`
- `POST /attribution/events`
- `POST /opportunities`
- `POST /content/assets`
- `POST /research/briefs`
- `POST /conversion/events`
- `POST /projects/tasks`
- `POST /approvals`
- `PATCH /approvals/{approval_id}/decision`
- `POST /tickets`
- `POST /meetings`

Domain routes require either `Authorization: Bearer <token>` or the legacy `Auth` header matching
`BENEFACTOR_MARKETING_API_AUTH_BEARER`.
