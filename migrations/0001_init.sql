-- 3FA sync server schema.
--
-- Zero-knowledge invariant: the server stores ONLY ciphertext for vault data.
-- There is no column anywhere here that holds a plaintext OTP seed or the user's
-- vault password. `auth_secret` is an Argon2id verifier (or, post-upgrade, an
-- OPAQUE envelope) from which the password cannot be recovered.

CREATE TABLE IF NOT EXISTS accounts (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username      TEXT NOT NULL UNIQUE,
    -- Argon2id PHC string today; OPAQUE registration envelope (bytea) later.
    auth_secret   TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS devices (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id       UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_name      TEXT NOT NULL,
    -- SHA-256 of the bearer sync token issued to this device.
    sync_token_hash  TEXT NOT NULL,
    revoked          BOOLEAN NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS devices_account_idx ON devices(account_id);
CREATE INDEX IF NOT EXISTS devices_token_idx ON devices(sync_token_hash);

-- One sealed vault blob per account (last-writer-wins-with-merge via the
-- version vector). All payload fields are opaque ciphertext / public KDF params.
CREATE TABLE IF NOT EXISTS vault_blobs (
    account_id   UUID PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    ciphertext   BYTEA NOT NULL,
    nonce        BYTEA NOT NULL,
    kdf_salt     BYTEA NOT NULL,
    kdf_params   JSONB NOT NULL,
    version      JSONB NOT NULL,         -- VersionVector
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
