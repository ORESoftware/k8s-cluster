-- billing-server-rs :: reconciliation + on-chain anchors

CREATE TYPE break_status AS ENUM (
    'open',
    'acknowledged',
    'auto_resolved',
    'resolved'
);

CREATE TABLE reconciliation_breaks (
    id                  BIGSERIAL PRIMARY KEY,
    tenant_id           UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key           BIGINT NOT NULL,
    provider            provider_kind NOT NULL,
    connection_id       UUID REFERENCES provider_connections(id) ON DELETE SET NULL,
    break_type          TEXT NOT NULL,
    external_ref        TEXT,
    transaction_id      UUID REFERENCES transactions(id) ON DELETE SET NULL,
    expected_minor      NUMERIC(38, 0),
    actual_minor        NUMERIC(38, 0),
    currency            CHAR(3),
    status              break_status NOT NULL DEFAULT 'open',
    notes               TEXT,
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at         TIMESTAMPTZ,
    metadata            JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX recon_breaks_tenant_open_idx ON reconciliation_breaks (tenant_id, status)
    WHERE status = 'open';
CREATE INDEX recon_breaks_shard_idx ON reconciliation_breaks (shard_key);

------------------------------------------------------------------------------
-- On-chain anchors. We periodically compute a Merkle root over a range of
-- postings and publish it to Solana (via a memo). The signature + slot let
-- anyone independently verify that a posting existed at a known point in time.
------------------------------------------------------------------------------

CREATE TABLE anchors (
    id                  BIGSERIAL PRIMARY KEY,
    tenant_id           UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key           BIGINT NOT NULL,
    from_posting_id     BIGINT NOT NULL,
    to_posting_id       BIGINT NOT NULL,
    posting_count       BIGINT NOT NULL,
    merkle_root         BYTEA NOT NULL,
    chain               TEXT NOT NULL DEFAULT 'solana',
    tx_signature        TEXT,
    slot                BIGINT,
    finalized_at        TIMESTAMPTZ,
    submitted_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, from_posting_id, to_posting_id)
);

CREATE INDEX anchors_tenant_idx ON anchors (tenant_id, submitted_at DESC);
CREATE INDEX anchors_unfinalized_idx ON anchors (submitted_at)
    WHERE finalized_at IS NULL;
