-- Reviewed additive contract for durable Daedalus fabrication work.
--
-- Canonical ownership:
--   ORESoftware/k8s-libs-and-shared-defs/pg-defs/schema/schema.sql
--
-- This file is documentation and review input. The application MUST NOT run it
-- at startup. Promote the definitions into the canonical shared schema,
-- regenerate adapters, review the diff, and apply through the normal RDS
-- deployment workflow before enabling the job-control workloads.

BEGIN;

CREATE TABLE daedalus.fabrication_job_executions (
    job_id uuid PRIMARY KEY,
    tenant_id text NOT NULL,
    request_id text NOT NULL,
    idempotency_key text NOT NULL,
    kind text NOT NULL,
    state text NOT NULL DEFAULT 'queued',
    current_stage text NOT NULL DEFAULT 'accepted',
    checkpoint_version bigint NOT NULL DEFAULT 0,
    checkpoint jsonb NOT NULL DEFAULT '{}'::jsonb,
    request_payload jsonb NOT NULL,
    result_payload jsonb,
    attempt_count integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 5,
    priority smallint NOT NULL DEFAULT 0,
    lease_owner text,
    lease_expires_at timestamptz,
    fiducia_fencing_token bigint,
    next_attempt_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    last_error_code text,
    last_error_message text,
    started_at timestamptz,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),

    CONSTRAINT fabrication_job_executions_tenant_id_nonempty
        CHECK (btrim(tenant_id) <> ''),
    CONSTRAINT fabrication_job_executions_request_id_nonempty
        CHECK (btrim(request_id) <> ''),
    CONSTRAINT fabrication_job_executions_idempotency_key_nonempty
        CHECK (btrim(idempotency_key) <> ''),
    CONSTRAINT fabrication_job_executions_kind_nonempty
        CHECK (btrim(kind) <> ''),
    CONSTRAINT fabrication_job_executions_state_valid
        CHECK (
            state IN (
                'queued',
                'running',
                'retry_wait',
                'succeeded',
                'failed',
                'cancelled'
            )
        ),
    CONSTRAINT fabrication_job_executions_checkpoint_version_nonnegative
        CHECK (checkpoint_version >= 0),
    CONSTRAINT fabrication_job_executions_attempts_valid
        CHECK (
            attempt_count >= 0
            AND max_attempts BETWEEN 1 AND 100
            AND attempt_count <= max_attempts
        ),
    CONSTRAINT fabrication_job_executions_fencing_token_nonnegative
        CHECK (
            fiducia_fencing_token IS NULL
            OR fiducia_fencing_token >= 0
        ),
    CONSTRAINT fabrication_job_executions_running_lease_complete
        CHECK (
            (state = 'running')
            = (
                lease_owner IS NOT NULL
                AND btrim(lease_owner) <> ''
                AND lease_expires_at IS NOT NULL
                AND fiducia_fencing_token IS NOT NULL
            )
        ),
    CONSTRAINT fabrication_job_executions_terminal_completed_at
        CHECK (
            state NOT IN ('succeeded', 'failed', 'cancelled')
            OR completed_at IS NOT NULL
        ),
    CONSTRAINT fabrication_job_executions_tenant_idempotency_unique
        UNIQUE (tenant_id, idempotency_key)
);

CREATE INDEX fabrication_job_executions_dispatch_idx
    ON daedalus.fabrication_job_executions (
        priority DESC,
        next_attempt_at,
        created_at
    )
    WHERE state IN ('queued', 'retry_wait');

CREATE INDEX fabrication_job_executions_expired_lease_idx
    ON daedalus.fabrication_job_executions (
        lease_expires_at,
        created_at
    )
    WHERE state = 'running';

CREATE INDEX fabrication_job_executions_request_idx
    ON daedalus.fabrication_job_executions (tenant_id, request_id);

CREATE TABLE daedalus.fabrication_job_outbox (
    event_id uuid PRIMARY KEY,
    job_id uuid NOT NULL
        REFERENCES daedalus.fabrication_job_executions(job_id),
    subject text NOT NULL,
    event_type text NOT NULL,
    message_id text NOT NULL,
    payload jsonb NOT NULL,
    available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    publish_attempts integer NOT NULL DEFAULT 0,
    claim_owner text,
    claim_expires_at timestamptz,
    published_at timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),

    CONSTRAINT fabrication_job_outbox_subject_nonempty
        CHECK (btrim(subject) <> ''),
    CONSTRAINT fabrication_job_outbox_subject_owned
        CHECK (subject LIKE 'dd.remote.fabrication.%'),
    CONSTRAINT fabrication_job_outbox_event_type_nonempty
        CHECK (btrim(event_type) <> ''),
    CONSTRAINT fabrication_job_outbox_message_id_nonempty
        CHECK (btrim(message_id) <> ''),
    CONSTRAINT fabrication_job_outbox_message_id_unique
        UNIQUE (message_id),
    CONSTRAINT fabrication_job_outbox_publish_attempts_nonnegative
        CHECK (publish_attempts >= 0),
    CONSTRAINT fabrication_job_outbox_claim_complete
        CHECK (
            (claim_owner IS NULL AND claim_expires_at IS NULL)
            OR (
                claim_owner IS NOT NULL
                AND btrim(claim_owner) <> ''
                AND claim_expires_at IS NOT NULL
            )
        ),
    CONSTRAINT fabrication_job_outbox_published_not_claimed
        CHECK (
            published_at IS NULL
            OR (claim_owner IS NULL AND claim_expires_at IS NULL)
        )
);

CREATE INDEX fabrication_job_outbox_ready_idx
    ON daedalus.fabrication_job_outbox (
        available_at,
        created_at,
        event_id
    )
    WHERE published_at IS NULL;

CREATE INDEX fabrication_job_outbox_job_idx
    ON daedalus.fabrication_job_outbox (job_id, created_at);

COMMENT ON TABLE daedalus.fabrication_job_executions IS
    'Canonical resumable state machine for fabrication work. NATS is transport, not ownership or recovery state.';

COMMENT ON COLUMN daedalus.fabrication_job_executions.checkpoint IS
    'Last fully committed idempotent recovery point. Never records an in-progress external side effect.';

COMMENT ON COLUMN daedalus.fabrication_job_executions.fiducia_fencing_token IS
    'Highest accepted Fiducia fencing token. A lower token may never mutate the job.';

COMMENT ON TABLE daedalus.fabrication_job_outbox IS
    'Transactional outbox for deterministic JetStream wakeups and terminal events.';

COMMIT;
