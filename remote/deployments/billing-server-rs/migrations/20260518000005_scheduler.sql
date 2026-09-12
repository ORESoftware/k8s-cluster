-- billing-server-rs :: durable scheduler (the "bulletproof cron")
--
-- Pattern: pg-boss / Sidekiq-PG / River. The runner loop does
--    SELECT ... FOR UPDATE SKIP LOCKED
-- to atomically claim due jobs, guaranteeing exactly-one execution per due
-- tick across N pods without any external coordination service.
--
-- Every run is recorded in job_runs (durable history). Failures are retried
-- with exponential backoff; after max_attempts a row is copied into
-- dead_letter_jobs so it surfaces on the breaks/ops dashboard.

CREATE TYPE schedule_kind AS ENUM ('cron', 'interval', 'one_shot');

CREATE TYPE job_run_status AS ENUM (
    'pending', 'claimed', 'succeeded', 'failed', 'dead_lettered', 'cancelled'
);

-- Tenant scope is optional so system jobs (lock sweeper, anchor sweeper)
-- can live in the same table as tenant jobs.
CREATE TABLE scheduled_jobs (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key           BIGINT NOT NULL DEFAULT 0,
    kind                TEXT NOT NULL,              -- e.g. "system.lock_sweeper", "tenant.payroll_run"
    name                TEXT NOT NULL,
    schedule_kind       schedule_kind NOT NULL,
    cron_expr           TEXT,                       -- when schedule_kind = 'cron'
    interval_seconds    INT,                        -- when schedule_kind = 'interval'
    one_shot_at         TIMESTAMPTZ,                -- when schedule_kind = 'one_shot'
    timezone            TEXT NOT NULL DEFAULT 'UTC',
    payload             JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled             BOOLEAN NOT NULL DEFAULT true,
    max_attempts        INT NOT NULL DEFAULT 5,
    retry_backoff_secs  INT NOT NULL DEFAULT 30,    -- base for exponential backoff
    timeout_seconds     INT NOT NULL DEFAULT 300,
    next_run_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_run_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One named definition per tenant per kind+name pair.
    UNIQUE NULLS NOT DISTINCT (tenant_id, kind, name)
);

CREATE INDEX scheduled_jobs_due_idx
    ON scheduled_jobs (enabled, next_run_at)
    WHERE enabled = true;
CREATE INDEX scheduled_jobs_tenant_idx ON scheduled_jobs (tenant_id);

CREATE TABLE job_runs (
    id                  BIGSERIAL PRIMARY KEY,
    job_id              UUID NOT NULL REFERENCES scheduled_jobs(id) ON DELETE CASCADE,
    tenant_id           UUID REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key           BIGINT NOT NULL DEFAULT 0,
    attempt             INT NOT NULL DEFAULT 1,
    status              job_run_status NOT NULL,
    scheduled_for       TIMESTAMPTZ NOT NULL,
    claimed_at          TIMESTAMPTZ,
    claimed_by          TEXT,                       -- pod / worker id
    finished_at         TIMESTAMPTZ,
    duration_ms         INT,
    output              JSONB,
    error               TEXT,
    idempotency_key     TEXT NOT NULL,
    UNIQUE (job_id, idempotency_key)
);

CREATE INDEX job_runs_job_idx       ON job_runs (job_id, scheduled_for DESC);
CREATE INDEX job_runs_tenant_idx    ON job_runs (tenant_id, finished_at DESC);
CREATE INDEX job_runs_status_idx    ON job_runs (status) WHERE status IN ('pending', 'claimed');

CREATE TABLE dead_letter_jobs (
    id                  BIGSERIAL PRIMARY KEY,
    job_id              UUID NOT NULL REFERENCES scheduled_jobs(id) ON DELETE CASCADE,
    tenant_id           UUID REFERENCES tenants(id) ON DELETE CASCADE,
    last_run_id         BIGINT REFERENCES job_runs(id) ON DELETE SET NULL,
    final_attempt       INT NOT NULL,
    error               TEXT,
    occurred_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    acknowledged_at     TIMESTAMPTZ
);

CREATE INDEX dead_letter_jobs_unack_idx
    ON dead_letter_jobs (occurred_at DESC)
    WHERE acknowledged_at IS NULL;
