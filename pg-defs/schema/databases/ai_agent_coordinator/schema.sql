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
-- Rust consumers use hand-written SeaORM entities in ai-agent-coordinator.rs.
-- These entities are runtime adapters only; this file is the schema authority.

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
