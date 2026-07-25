-- Declarative Postgres schema contract for the dd-build-server service.
--
-- NAMESPACE: this contract targets the build server's OWN database
-- (conventionally named `dd_build_server` on the shared Amazon RDS instance),
-- NOT the shared pg-defs database described by ../../schema.sql. Build-server
-- tables live in their own database namespace, following the own-database
-- pattern used by billing-server-rs and dd-embeddings-rs — the contract file
-- simply lives here in k8s-libs-and-shared-defs so all shared defs stay in one
-- repo.
--
-- Migrations are declarative via dpm (declarative-postgres-migrate):
--   remote/deployments/build-server-rs/scripts/dpm.sh {diff|verify|review|apply}
-- with this file as --source. Never apply this file directly to a live
-- database and never migrate at boot; generate and review a diff instead.
--
-- Rust consumers use hand-written SeaORM entities in
-- remote/deployments/build-server-rs/src/entity (pg-defs adapter style):
-- do not generate SQL or migrations from application code.

-- Build jobs: durable record of every accepted build-server.v1 job.
-- The in-memory job map in dd-build-server is a cache; this table is the
-- source of truth across restarts. Jobs found 'queued'/'running' at boot are
-- marked failed ('interrupted by restart').
create table if not exists build_jobs (
  id text primary key,
  status text not null default 'queued',
  job_kind text not null default 'build-image',
  -- Where the job came from: http | webhook | nats.
  source text not null default 'http',
  -- Which executor ran it: local (git+nerdctl+kubectl) | lambda (gleam-lambda-runner).
  executor text not null default 'local',
  repo_url text not null,
  git_ref text,
  image text not null,
  -- Full validated build-server.v1 request document (secrets are rejected at
  -- validation time, so this is safe to persist).
  request jsonb not null,
  error text,
  log_path text,
  -- fiducia.cloud coordination: the union-lock key guarding this build and the
  -- monotonic fencing token from the grant (null when coordination is off).
  lock_key text,
  fencing_token bigint,
  created_at timestamptz not null default now(),
  started_at timestamptz,
  finished_at timestamptz,
  constraint build_jobs_status_chk
    check (status in ('queued', 'running', 'succeeded', 'failed')),
  constraint build_jobs_job_kind_chk
    check (job_kind in ('build-image', 'build-and-deploy', 'run-profile')),
  constraint build_jobs_source_chk
    check (source in ('http', 'webhook', 'nats')),
  constraint build_jobs_executor_chk
    check (executor in ('local', 'lambda'))
);

create index if not exists build_jobs_status_idx
  on build_jobs (status);

create index if not exists build_jobs_created_at_idx
  on build_jobs (created_at desc);

create index if not exists build_jobs_repo_url_idx
  on build_jobs (repo_url);

-- Webhook deliveries: dedupe + audit log for inbound webhooks
-- (GitHub X-GitHub-Delivery GUIDs and registry event ids). The unique
-- constraint is the idempotency guard for at-least-once webhook redelivery.
create table if not exists webhook_deliveries (
  id bigint generated always as identity primary key,
  -- github | registry
  provider text not null,
  delivery_id text not null,
  event_kind text,
  repo text,
  git_ref text,
  -- What the server did with it: enqueued:<jobId> | ignored:<reason> | duplicate.
  action text not null,
  received_at timestamptz not null default now(),
  constraint webhook_deliveries_provider_chk
    check (provider in ('github', 'registry')),
  constraint webhook_deliveries_provider_delivery_uniq
    unique (provider, delivery_id)
);

create index if not exists webhook_deliveries_received_at_idx
  on webhook_deliveries (received_at desc);

-- GitHub Actions secret sync audit: one row per (repo, secret) sync attempt.
-- NEVER stores secret values — only the SHA-256 of the value that was synced,
-- so unchanged values can be skipped and drift can be detected.
create table if not exists gh_secret_sync_runs (
  id bigint generated always as identity primary key,
  repo text not null,
  secret_name text not null,
  value_sha256 text not null,
  -- synced | skipped-unchanged | failed
  status text not null,
  detail text,
  synced_at timestamptz not null default now(),
  constraint gh_secret_sync_runs_status_chk
    check (status in ('synced', 'skipped-unchanged', 'failed'))
);

create index if not exists gh_secret_sync_runs_repo_secret_idx
  on gh_secret_sync_runs (repo, secret_name, synced_at desc);
