-- billing-server-rs :: provider connections & OAuth state
--
-- Tenants connect their payment provider accounts via OAuth (where supported)
-- or by submitting API keys / bank coordinates (otherwise). The raw secret
-- material is sealed with an AES-GCM key wrapped by the tenant's KMS data key
-- and stored as a JSONB blob alongside the wrapping context.

CREATE TYPE provider_kind AS ENUM (
    'stripe',
    'paypal',
    'braintree',
    'coinbase_commerce',
    'coinbase_prime',
    'plaid_bank',
    'swift_wire',
    'ach_direct',
    'wise',
    'solana_wallet'
);

CREATE TYPE provider_auth_kind AS ENUM (
    'oauth2',
    'api_key',
    'bank_coordinates',
    'wallet_pubkey'
);

CREATE TYPE connection_status AS ENUM (
    'pending',
    'active',
    'token_refresh_failed',
    'revoked',
    'expired'
);

CREATE TABLE provider_connections (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    shard_key           BIGINT NOT NULL,
    provider            provider_kind NOT NULL,
    auth_kind           provider_auth_kind NOT NULL,
    external_account_id TEXT,
    display_label       TEXT NOT NULL,
    status              connection_status NOT NULL DEFAULT 'pending',

    -- Encrypted credential envelope. Plaintext shape is provider-specific.
    -- AES-256-GCM ciphertext (base64), nonce (base64), and AAD includes
    -- tenant_id + provider so a wrap from tenant A cannot be replayed for B.
    sealed_credential   JSONB,
    kms_key_version     INT NOT NULL DEFAULT 1,

    -- OAuth-specific metadata kept in cleartext for operational queries.
    scopes              TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    expires_at          TIMESTAMPTZ,
    refreshed_at        TIMESTAMPTZ,

    last_sync_at        TIMESTAMPTZ,
    last_sync_cursor    TEXT,
    last_error          TEXT,

    metadata            JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A tenant may have multiple connections per provider (e.g. 10 banks via
    -- plaid_bank), so uniqueness is by (tenant, provider, external_account_id)
    -- and only when external_account_id is known.
    UNIQUE (tenant_id, provider, external_account_id)
);

CREATE INDEX provider_connections_tenant_idx ON provider_connections (tenant_id);
CREATE INDEX provider_connections_shard_idx ON provider_connections (shard_key);
CREATE INDEX provider_connections_status_idx ON provider_connections (status)
    WHERE status IN ('token_refresh_failed', 'expired');

------------------------------------------------------------------------------
-- OAuth state (anti-CSRF nonce store for the OAuth handshake)
------------------------------------------------------------------------------

CREATE TABLE oauth_states (
    state           TEXT PRIMARY KEY,
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider        provider_kind NOT NULL,
    return_to       TEXT,
    pkce_verifier   TEXT,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX oauth_states_expires_idx ON oauth_states (expires_at);

------------------------------------------------------------------------------
-- Raw webhook events (kept for replay + audit)
------------------------------------------------------------------------------

CREATE TABLE webhook_events (
    id                  BIGSERIAL PRIMARY KEY,
    connection_id       UUID REFERENCES provider_connections(id) ON DELETE SET NULL,
    tenant_id           UUID REFERENCES tenants(id) ON DELETE CASCADE,
    provider            provider_kind NOT NULL,
    external_event_id   TEXT NOT NULL,
    event_type          TEXT NOT NULL,
    payload             JSONB NOT NULL,
    signature_ok        BOOLEAN NOT NULL,
    processed_at        TIMESTAMPTZ,
    process_error       TEXT,
    received_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, external_event_id)
);

CREATE INDEX webhook_events_tenant_idx ON webhook_events (tenant_id, received_at DESC);
CREATE INDEX webhook_events_unprocessed_idx ON webhook_events (received_at)
    WHERE processed_at IS NULL;
