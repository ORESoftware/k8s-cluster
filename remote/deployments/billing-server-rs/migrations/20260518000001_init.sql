-- billing-server-rs :: initial schema
--
-- Conventions:
--   * All money is stored in MINOR units as NUMERIC(38, 0). Never floats.
--   * Every tenant-scoped row carries shard_key for future horizontal partitioning.
--   * shard_key is derived from (tenant_id, region) and computed by the application.
--   * Postings table is append-only; UPDATE/DELETE is forbidden by trigger.
--   * Every posting carries idempotency_key + (source, source_event_id) for replay safety.

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS citext;

------------------------------------------------------------------------------
-- Tenants (the B2B customers of the billing server, e.g. dancingdragons.cc)
------------------------------------------------------------------------------

CREATE TABLE tenants (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug            CITEXT NOT NULL UNIQUE,
    display_name    TEXT   NOT NULL,
    country_code    CHAR(2) NOT NULL,
    us_state        CHAR(2),
    base_currency   CHAR(3) NOT NULL DEFAULT 'USD',
    kms_key_id      TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'suspended', 'terminated')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX tenants_country_idx ON tenants (country_code);

------------------------------------------------------------------------------
-- Tenant API keys (tenants authenticate to us; one tenant can have many keys)
------------------------------------------------------------------------------

CREATE TABLE tenant_api_keys (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key_prefix      TEXT NOT NULL UNIQUE,
    key_hash        BYTEA NOT NULL,
    label           TEXT NOT NULL,
    scopes          TEXT[] NOT NULL DEFAULT ARRAY['read', 'write'],
    last_used_at    TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX tenant_api_keys_tenant_idx ON tenant_api_keys (tenant_id);

------------------------------------------------------------------------------
-- Users (end-users / counterparties of a tenant)
-- A user is uniquely identified by (tenant_id, email).
-- A user can be a customer (we bill them), a vendor (we pay them), or both.
------------------------------------------------------------------------------

CREATE TABLE users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key       BIGINT NOT NULL,
    email           CITEXT NOT NULL,
    display_name    TEXT,
    country_code    CHAR(2),
    us_state        CHAR(2),
    is_customer     BOOLEAN NOT NULL DEFAULT false,
    is_vendor       BOOLEAN NOT NULL DEFAULT false,
    external_refs   JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, email)
);

CREATE INDEX users_shard_idx ON users (shard_key);
CREATE INDEX users_tenant_idx ON users (tenant_id);
CREATE INDEX users_external_refs_gin ON users USING gin (external_refs);

------------------------------------------------------------------------------
-- Accounts (ledger primitive). Multiple accounts per user.
-- Examples: ar/cus_x, ap/ven_y, clearing/stripe, cash/chase, onchain/sol_wallet
------------------------------------------------------------------------------

CREATE TYPE account_kind AS ENUM (
    'asset',        -- cash, clearing, onchain
    'liability',    -- accounts_payable, unallocated_cash
    'income',       -- revenue
    'expense',      -- fees, chargebacks
    'receivable'    -- accounts_receivable per customer
);

CREATE TYPE account_normal_side AS ENUM ('debit', 'credit');

CREATE TABLE accounts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    shard_key       BIGINT NOT NULL,
    user_id         UUID REFERENCES users(id) ON DELETE RESTRICT,
    kind            account_kind NOT NULL,
    normal_side     account_normal_side NOT NULL,
    code            TEXT NOT NULL,
    currency        CHAR(3) NOT NULL,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, code, currency)
);

CREATE INDEX accounts_shard_idx ON accounts (shard_key);
CREATE INDEX accounts_tenant_idx ON accounts (tenant_id);
CREATE INDEX accounts_user_idx ON accounts (user_id) WHERE user_id IS NOT NULL;

------------------------------------------------------------------------------
-- Transactions (header) + postings (rows)
-- A transaction is a set of N>=2 postings that MUST sum to zero per currency.
------------------------------------------------------------------------------

CREATE TABLE transactions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    shard_key       BIGINT NOT NULL,
    kind            TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    description     TEXT,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    posted_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, idempotency_key)
);

CREATE INDEX transactions_shard_idx ON transactions (shard_key);
CREATE INDEX transactions_tenant_posted_idx ON transactions (tenant_id, posted_at DESC);

CREATE TYPE posting_direction AS ENUM ('debit', 'credit');

CREATE TABLE postings (
    id                BIGSERIAL PRIMARY KEY,
    transaction_id    UUID NOT NULL REFERENCES transactions(id) ON DELETE RESTRICT,
    tenant_id         UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    shard_key         BIGINT NOT NULL,
    account_id        UUID NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    direction         posting_direction NOT NULL,
    amount_minor      NUMERIC(38, 0) NOT NULL CHECK (amount_minor > 0),
    currency          CHAR(3) NOT NULL,
    source            TEXT NOT NULL,
    source_event_id   TEXT NOT NULL,
    posted_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    metadata          JSONB NOT NULL DEFAULT '{}'::jsonb,
    UNIQUE (source, source_event_id, direction, account_id)
);

CREATE INDEX postings_shard_idx ON postings (shard_key);
CREATE INDEX postings_tx_idx ON postings (transaction_id);
CREATE INDEX postings_account_idx ON postings (account_id, posted_at DESC);
CREATE INDEX postings_tenant_posted_idx ON postings (tenant_id, posted_at DESC);

-- Append-only enforcement
CREATE OR REPLACE FUNCTION postings_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'postings are append-only; UPDATE/DELETE forbidden';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER postings_no_update BEFORE UPDATE ON postings
    FOR EACH ROW EXECUTE FUNCTION postings_immutable();
CREATE TRIGGER postings_no_delete BEFORE DELETE ON postings
    FOR EACH ROW EXECUTE FUNCTION postings_immutable();

-- Per-transaction zero-sum invariant, checked at COMMIT time.
CREATE OR REPLACE FUNCTION transactions_must_balance() RETURNS trigger AS $$
DECLARE
    bad RECORD;
BEGIN
    FOR bad IN
        SELECT t.id, p.currency,
               SUM(CASE WHEN p.direction = 'debit' THEN p.amount_minor ELSE -p.amount_minor END) AS net
        FROM transactions t
        JOIN postings p ON p.transaction_id = t.id
        WHERE t.id = NEW.id
        GROUP BY t.id, p.currency
        HAVING SUM(CASE WHEN p.direction = 'debit' THEN p.amount_minor ELSE -p.amount_minor END) <> 0
    LOOP
        RAISE EXCEPTION 'transaction % is not balanced in currency %: net=%',
                        bad.id, bad.currency, bad.net;
    END LOOP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER transactions_balance_check
    AFTER INSERT ON transactions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION transactions_must_balance();
