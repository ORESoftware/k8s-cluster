-- billing-server-rs :: tenant-scoped leases (the lock primitive).
--
-- Leases (not strict mutexes) — every acquire has a TTL so a crashed client
-- cannot hold a lock forever. Callers receive an opaque `lease_token` UUID;
-- renew and release require presenting that token so a third party that
-- merely knows the resource_key cannot steal the lease.
--
-- Backed by Postgres so failover comes "for free" via the same PG HA story
-- used by the ledger. No separate distributed-lock infrastructure required.

CREATE TABLE tenant_locks (
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key     BIGINT      NOT NULL,
    resource_key  TEXT        NOT NULL,
    lease_token   UUID        NOT NULL,
    holder        TEXT,
    acquired_at   TIMESTAMPTZ NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    metadata      JSONB       NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (tenant_id, resource_key)
);

CREATE INDEX tenant_locks_shard_idx ON tenant_locks (shard_key);
-- Purge sweeper queries this index.
CREATE INDEX tenant_locks_expired_idx ON tenant_locks (expires_at);

-- Audit trail of every acquire/renew/release. Append-only, retained 90 days
-- by a background job (not yet written). This is the SOC 2 control surface
-- for the lock feature.
CREATE TYPE lock_event_kind AS ENUM ('acquire', 'renew', 'release', 'preempt', 'expire');

CREATE TABLE tenant_lock_events (
    id            BIGSERIAL PRIMARY KEY,
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key     BIGINT      NOT NULL,
    resource_key  TEXT        NOT NULL,
    lease_token   UUID,
    kind          lock_event_kind NOT NULL,
    holder        TEXT,
    actor         TEXT,                   -- API caller (e.g. "tenant-api-key:tak_abc")
    ttl_seconds   INT,
    occurred_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    metadata      JSONB       NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX tenant_lock_events_tenant_idx
    ON tenant_lock_events (tenant_id, resource_key, occurred_at DESC);
