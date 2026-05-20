-- billing-server-rs :: sync hardening
--
-- Provider sync runs on many pods and many tenants. These tables/indexes keep
-- the shared concurrency and idempotency rules in Postgres so workers can fail
-- and recover without double-posting or accidentally stampeding providers.

------------------------------------------------------------------------------
-- Tenant-scoped provider event idempotency
------------------------------------------------------------------------------

-- The initial scaffold keyed source events globally. Provider event ids are not
-- a safe cross-tenant namespace, especially for bank files and synthetic import
-- ids. Keep replay protection, but scope it to the tenant ledger.
ALTER TABLE postings
    DROP CONSTRAINT IF EXISTS postings_source_source_event_id_direction_account_id_key;

CREATE UNIQUE INDEX IF NOT EXISTS postings_tenant_source_event_direction_account_uq
    ON postings (tenant_id, source, source_event_id, direction, account_id);

------------------------------------------------------------------------------
-- Connection sync state and scheduler lookup support
------------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS provider_connections_due_sync_idx
    ON provider_connections (tenant_id, provider, last_sync_at)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS provider_connections_cursor_idx
    ON provider_connections (id, last_sync_cursor)
    WHERE status = 'active';

------------------------------------------------------------------------------
-- Distributed provider request budgets
------------------------------------------------------------------------------

CREATE TABLE provider_rate_limit_buckets (
    tenant_id        UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider         provider_kind NOT NULL,
    window_start     TIMESTAMPTZ NOT NULL,
    window_seconds   INT NOT NULL CHECK (window_seconds > 0),
    request_limit    INT NOT NULL CHECK (request_limit > 0),
    requests_used    INT NOT NULL DEFAULT 0 CHECK (requests_used >= 0),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, provider, window_start, window_seconds)
);

CREATE INDEX provider_rate_limit_buckets_gc_idx
    ON provider_rate_limit_buckets (window_start);
