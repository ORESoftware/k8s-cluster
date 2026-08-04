-- Declarative Postgres schema contract for ai-agent-coordinator.rs.
--
-- NAMESPACE: every owned object lives in the `ai_agent_coordinator` Postgres
-- schema. The coordinator may share a database with other applications without
-- putting its tables in `public` or relying on search_path.
--
-- Migrations are declarative via dpm (declarative-postgres-migrate):
--   ai-agent-coordinator.rs/scripts/dpm.sh {diff|verify|review|apply}
-- with this file as --source and --schemas ai_agent_coordinator. Never apply
-- this file directly to a live database and never migrate at application boot.
--
-- Rust consumers use hand-written SeaORM entities or schema-qualified SeaORM
-- statements in ai-agent-coordinator.rs. Those are runtime adapters only; this
-- file is the schema authority.

create schema if not exists ai_agent_coordinator;

-- Durable leased queue for agent work.
create table if not exists ai_agent_coordinator.jobs (
  id text primary key,
  org text not null,
  repo text not null,
  task_type text not null,
  payload jsonb not null,
  priority bigint not null default 0,
  status text not null default 'queued',
  idempotency_key text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  available_at timestamptz not null default now(),
  claimed_by text,
  lease_expires_at timestamptz,
  attempts bigint not null default 0,
  max_attempts bigint not null default 3,
  result jsonb,
  last_error text,
  budget_usd double precision,
  constraint jobs_idempotency_key_unique unique (idempotency_key),
  constraint jobs_status_chk
    check (status in ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
  constraint jobs_priority_chk check (priority between -1000 and 1000),
  constraint jobs_attempts_chk check (attempts >= 0),
  constraint jobs_max_attempts_chk check (max_attempts between 1 and 100),
  constraint jobs_budget_usd_chk check (budget_usd is null or budget_usd > 0),
  constraint jobs_slack_agent_run_idempotency_chk check (
    task_type <> 'slack_agent_run'
    or (
      idempotency_key is not null
      and idempotency_key ~ '^ores-[0-9a-f]{24}$'
      and jsonb_typeof(payload) = 'object'
      and payload ->> 'schema_version' = '1'
      and payload ->> 'run_id' = idempotency_key
    )
  ),
  constraint jobs_running_lease_chk check (
    status <> 'running'
    or (claimed_by is not null and lease_expires_at is not null)
  )
);

create index if not exists jobs_claim_idx
  on ai_agent_coordinator.jobs
  (status, available_at, priority desc, created_at asc);

create index if not exists jobs_repo_idx
  on ai_agent_coordinator.jobs
  (org, repo, status, created_at desc);

create index if not exists jobs_running_org_idx
  on ai_agent_coordinator.jobs (org)
  where status = 'running';

create index if not exists jobs_running_repo_idx
  on ai_agent_coordinator.jobs (org, repo)
  where status = 'running';

-- Per-request model token and cost ledger used for daily budget enforcement.
create table if not exists ai_agent_coordinator.model_usage (
  id bigint generated always as identity primary key,
  request_id text not null,
  created_at timestamptz not null default now(),
  org text not null,
  repo text not null,
  provider text not null,
  model text not null,
  prompt_tokens bigint not null,
  completion_tokens bigint not null,
  cost_usd double precision not null,
  constraint model_usage_prompt_tokens_chk check (prompt_tokens >= 0),
  constraint model_usage_completion_tokens_chk check (completion_tokens >= 0),
  constraint model_usage_cost_usd_chk check (cost_usd >= 0)
);

create index if not exists model_usage_org_time_idx
  on ai_agent_coordinator.model_usage (org, created_at);

create index if not exists model_usage_repo_time_idx
  on ai_agent_coordinator.model_usage (org, repo, created_at);

-- Idempotency ledger for externally visible Linear mutations.
create table if not exists ai_agent_coordinator.linear_mutations (
  mutation_key text primary key,
  job_id text not null references ai_agent_coordinator.jobs (id) on delete cascade,
  organization text not null,
  repository text not null,
  issue_identifier text not null,
  commit_id text not null,
  keyword text not null,
  action text not null,
  status text not null default 'pending',
  attempts bigint not null default 0,
  last_error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint linear_mutations_action_chk
    check (action in ('reference', 'reference_and_transition')),
  constraint linear_mutations_status_chk
    check (status in ('pending', 'succeeded', 'failed')),
  constraint linear_mutations_attempts_chk check (attempts >= 0)
);

create index if not exists linear_mutations_status_idx
  on ai_agent_coordinator.linear_mutations (status, updated_at);

create index if not exists linear_mutations_job_id_idx
  on ai_agent_coordinator.linear_mutations (job_id);

-- Read-only inbox scan cursors and redacted source health.
create table if not exists ai_agent_coordinator.email_attention_sources (
  source_id text primary key,
  provider text not null,
  cursor text,
  last_success_at timestamptz,
  last_error text,
  last_error_at timestamptz,
  updated_at timestamptz not null default now(),
  constraint email_attention_sources_source_id_chk
    check (source_id ~ '^[A-Za-z0-9._-]{1,64}$'),
  constraint email_attention_sources_provider_chk
    check (provider in ('gmail', 'outlook')),
  constraint email_attention_sources_cursor_chk
    check (cursor is null or char_length(cursor) between 1 and 4096),
  constraint email_attention_sources_last_error_chk
    check (last_error is null or char_length(last_error) <= 512)
);

-- Durable outbox. The payload contains only the bounded user-visible digest and
-- is replaced with {"redacted":true} after confirmed delivery.
create table if not exists ai_agent_coordinator.email_attention_deliveries (
  idempotency_key text primary key,
  payload_json jsonb not null,
  status text not null default 'pending',
  attempts bigint not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  delivered_at timestamptz,
  last_error text,
  constraint email_attention_deliveries_key_chk
    check (char_length(idempotency_key) between 1 and 256),
  constraint email_attention_deliveries_status_chk
    check (status in ('pending', 'delivered')),
  constraint email_attention_deliveries_attempts_chk check (attempts >= 0),
  constraint email_attention_deliveries_last_error_chk
    check (last_error is null or char_length(last_error) <= 512)
);

create index if not exists email_attention_deliveries_pending_idx
  on ai_agent_coordinator.email_attention_deliveries
  (status, created_at)
  where status = 'pending';

-- Message/thread identity, material-change fingerprint, and suppression state.
-- Sender, subject, snippets, bodies, attachments, and raw mailbox addresses do
-- not belong in this table.
create table if not exists ai_agent_coordinator.email_attention_items (
  source_id text not null,
  stable_id text not null,
  current_fingerprint text not null,
  current_bucket text not null,
  deadline_at timestamptz,
  last_seen_at timestamptz not null,
  last_emitted_fingerprint text,
  last_emitted_at timestamptz,
  pending_delivery_key text,
  primary key (source_id, stable_id),
  constraint email_attention_items_source_id_fk
    foreign key (source_id)
    references ai_agent_coordinator.email_attention_sources (source_id)
    on delete cascade,
  constraint email_attention_items_pending_delivery_fk
    foreign key (pending_delivery_key)
    references ai_agent_coordinator.email_attention_deliveries (idempotency_key)
    on delete set null,
  constraint email_attention_items_stable_id_chk
    check (char_length(stable_id) between 1 and 512),
  constraint email_attention_items_current_fingerprint_chk
    check (current_fingerprint ~ '^[0-9a-f]{64}$'),
  constraint email_attention_items_last_emitted_fingerprint_chk
    check (
      last_emitted_fingerprint is null
      or last_emitted_fingerprint ~ '^[0-9a-f]{64}$'
    ),
  constraint email_attention_items_bucket_chk
    check (current_bucket in ('urgent', 'needs_reply_soon')),
  constraint email_attention_items_emitted_pair_chk
    check (
      (last_emitted_fingerprint is null and last_emitted_at is null)
      or (last_emitted_fingerprint is not null and last_emitted_at is not null)
    )
);

create index if not exists email_attention_items_pending_idx
  on ai_agent_coordinator.email_attention_items (pending_delivery_key)
  where pending_delivery_key is not null;

create index if not exists email_attention_items_deadline_idx
  on ai_agent_coordinator.email_attention_items (deadline_at)
  where deadline_at is not null;

-- Exact fingerprints carried by each outbox delivery. This decouples a
-- delivered historical fingerprint from any newer material change observed
-- for the same message before the delivery completes.
create table if not exists ai_agent_coordinator.email_attention_delivery_items (
  idempotency_key text not null,
  source_id text not null,
  stable_id text not null,
  fingerprint text not null,
  primary key (idempotency_key, source_id, stable_id),
  constraint email_attention_delivery_items_delivery_fk
    foreign key (idempotency_key)
    references ai_agent_coordinator.email_attention_deliveries (idempotency_key)
    on delete cascade,
  constraint email_attention_delivery_items_item_fk
    foreign key (source_id, stable_id)
    references ai_agent_coordinator.email_attention_items (source_id, stable_id)
    on delete cascade,
  constraint email_attention_delivery_items_fingerprint_chk
    check (fingerprint ~ '^[0-9a-f]{64}$')
);

-- Aggregate run history; no message body or snippet content is retained.
create table if not exists ai_agent_coordinator.email_attention_runs (
  run_id text primary key,
  mode text not null,
  started_at timestamptz not null,
  finished_at timestamptz not null,
  scan_status text not null,
  notification_status text not null,
  attention_item_count bigint not null,
  source_success_count bigint not null,
  source_failure_count bigint not null,
  error text,
  constraint email_attention_runs_mode_chk
    check (mode in ('scheduled', 'manual_test')),
  constraint email_attention_runs_scan_status_chk
    check (scan_status in ('success', 'partial', 'failed')),
  constraint email_attention_runs_counts_chk
    check (
      attention_item_count >= 0
      and source_success_count >= 0
      and source_failure_count >= 0
    ),
  constraint email_attention_runs_error_chk
    check (error is null or char_length(error) <= 512),
  constraint email_attention_runs_time_chk
    check (finished_at >= started_at)
);

create index if not exists email_attention_runs_finished_idx
  on ai_agent_coordinator.email_attention_runs (finished_at desc);

-- Compare-and-swap lease used to keep multiple coordinator replicas from
-- running the same scheduled scan concurrently.
create table if not exists ai_agent_coordinator.email_attention_leases (
  name text primary key,
  holder text not null,
  expires_at timestamptz not null,
  updated_at timestamptz not null default now(),
  constraint email_attention_leases_name_chk
    check (char_length(name) between 1 and 128),
  constraint email_attention_leases_holder_chk
    check (char_length(holder) between 1 and 128)
);

create index if not exists email_attention_leases_expiry_idx
  on ai_agent_coordinator.email_attention_leases (expires_at);
