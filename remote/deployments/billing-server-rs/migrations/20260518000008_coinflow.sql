-- billing-server-rs :: add Coinflow as a provider_kind value.
--
-- Coinflow (https://coinflow.cash) is a Polish-registered VASP that does
-- both fiat (card/ACH/Cash App) and crypto (Solana settlement) under one
-- API. We connect via API key + merchant id (not OAuth) and read history
-- via /api/merchant/webhooks + /api/customer/history + /api/withdraw/history.
--
-- Important: PG12+ permits ALTER TYPE ... ADD VALUE in a transaction as long
-- as the value is not referenced within the same transaction. This migration
-- only adds the value; first row writes happen in user-driven traffic.

ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'coinflow';

-- We also want a stable place to record Coinflow's "wallet balance" snapshots
-- (their dashboard shows a per-merchant wallet balance with APY accrual).
-- Not used yet, but reserved so the wallet-balance reconciler doesn't have
-- to add another migration later.
CREATE TABLE IF NOT EXISTS provider_balance_snapshots (
    id              BIGSERIAL PRIMARY KEY,
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key       BIGINT NOT NULL,
    connection_id   UUID NOT NULL REFERENCES provider_connections(id) ON DELETE CASCADE,
    currency        CHAR(3) NOT NULL,
    available_minor NUMERIC(38, 0) NOT NULL,
    pending_minor   NUMERIC(38, 0) NOT NULL DEFAULT 0,
    apy_bps         INT,
    snapshot_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    raw             JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS pbs_connection_idx
    ON provider_balance_snapshots (connection_id, snapshot_at DESC);
