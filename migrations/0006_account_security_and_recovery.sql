-- Device lifecycle, backup recovery channels, and opaque recovery packages.
--
-- Security invariant: this schema never stores a PIN, PIN verifier, biometric
-- template, platform private key, Signal private key, OTP code, account master
-- key, or decrypted recovery package. Contact destinations are envelope-
-- encrypted with a service/KMS key and have a keyed blind digest for uniqueness.

ALTER TABLE threefa.devices
    ADD COLUMN IF NOT EXISTS lifecycle_state TEXT NOT NULL DEFAULT 'active',
    ADD COLUMN IF NOT EXISTS platform TEXT NOT NULL DEFAULT 'unknown',
    ADD COLUMN IF NOT EXISTS identity_key_fingerprint_base64 TEXT,
    ADD COLUMN IF NOT EXISTS local_unlock_policy JSONB NOT NULL DEFAULT '{"pin_enabled":false,"biometric_enabled":false,"passkey_enabled":false}'::jsonb,
    ADD COLUMN IF NOT EXISTS verified_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS suspended_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS revoked_at TIMESTAMPTZ;

UPDATE threefa.devices
SET lifecycle_state = CASE WHEN revoked THEN 'revoked' ELSE lifecycle_state END,
    revoked_at = CASE WHEN revoked AND revoked_at IS NULL THEN now() ELSE revoked_at END;

ALTER TABLE threefa.devices
    DROP CONSTRAINT IF EXISTS devices_lifecycle_state_valid;
ALTER TABLE threefa.devices
    ADD CONSTRAINT devices_lifecycle_state_valid CHECK (
        lifecycle_state IN ('pending', 'active', 'suspended', 'revoked')
    );

ALTER TABLE threefa.devices
    DROP CONSTRAINT IF EXISTS devices_lifecycle_timestamps_consistent;
ALTER TABLE threefa.devices
    ADD CONSTRAINT devices_lifecycle_timestamps_consistent CHECK (
        (lifecycle_state = 'revoked' AND revoked_at IS NOT NULL)
        OR (lifecycle_state <> 'revoked' AND revoked_at IS NULL)
    );

ALTER TABLE threefa.devices
    DROP CONSTRAINT IF EXISTS devices_identity_fingerprint_bounded;
ALTER TABLE threefa.devices
    ADD CONSTRAINT devices_identity_fingerprint_bounded CHECK (
        identity_key_fingerprint_base64 IS NULL
        OR octet_length(identity_key_fingerprint_base64) BETWEEN 16 AND 512
    );

CREATE INDEX IF NOT EXISTS devices_account_lifecycle_idx
    ON threefa.devices (account_id, lifecycle_state, created_at);

CREATE TABLE IF NOT EXISTS threefa.recovery_channels (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id               UUID NOT NULL REFERENCES threefa.accounts(id) ON DELETE CASCADE,
    kind                     TEXT NOT NULL CHECK (kind IN ('email', 'phone')),
    destination_ciphertext   TEXT NOT NULL,
    destination_key_id       TEXT NOT NULL,
    destination_blind_digest TEXT NOT NULL,
    masked_destination       TEXT NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    verified_at              TIMESTAMPTZ,
    disabled_at              TIMESTAMPTZ,
    CONSTRAINT recovery_channel_ciphertext_bounded CHECK (
        octet_length(destination_ciphertext) BETWEEN 16 AND 4096
        AND octet_length(destination_key_id) BETWEEN 1 AND 128
        AND octet_length(destination_blind_digest) BETWEEN 32 AND 256
        AND char_length(masked_destination) BETWEEN 3 AND 320
    ),
    CONSTRAINT recovery_channel_disabled_after_creation CHECK (
        disabled_at IS NULL OR disabled_at >= created_at
    ),
    UNIQUE (account_id, kind, destination_blind_digest)
);

CREATE INDEX IF NOT EXISTS recovery_channels_active_idx
    ON threefa.recovery_channels (account_id, kind, created_at)
    WHERE disabled_at IS NULL;

CREATE TABLE IF NOT EXISTS threefa.recovery_challenges (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id          UUID NOT NULL REFERENCES threefa.accounts(id) ON DELETE CASCADE,
    channel_id          UUID NOT NULL REFERENCES threefa.recovery_channels(id) ON DELETE CASCADE,
    purpose             TEXT NOT NULL CHECK (
        purpose IN ('add_device', 'account_recovery', 'step_up', 'change_channel', 'revoke_device')
    ),
    code_digest         TEXT NOT NULL,
    digest_key_id       TEXT NOT NULL,
    attempts_remaining  SMALLINT NOT NULL DEFAULT 5 CHECK (attempts_remaining BETWEEN 0 AND 10),
    requested_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at          TIMESTAMPTZ NOT NULL,
    consumed_at         TIMESTAMPTZ,
    invalidated_at      TIMESTAMPTZ,
    requester_risk_hash TEXT,
    CONSTRAINT recovery_challenge_digest_bounded CHECK (
        octet_length(code_digest) BETWEEN 32 AND 256
        AND octet_length(digest_key_id) BETWEEN 1 AND 128
    ),
    CONSTRAINT recovery_challenge_lifetime_bounded CHECK (
        expires_at > requested_at
        AND expires_at <= requested_at + INTERVAL '15 minutes'
    ),
    CONSTRAINT recovery_challenge_terminal_state CHECK (
        consumed_at IS NULL OR invalidated_at IS NULL
    )
);

CREATE INDEX IF NOT EXISTS recovery_challenges_active_idx
    ON threefa.recovery_challenges (account_id, channel_id, expires_at)
    WHERE consumed_at IS NULL AND invalidated_at IS NULL;

-- Optional zero-knowledge recovery. Email/SMS/passkey/biometric verification may
-- authorize retrieval, but only the user's recovery key can decrypt this blob.
CREATE TABLE IF NOT EXISTS threefa.encrypted_recovery_packages (
    account_id             UUID PRIMARY KEY REFERENCES threefa.accounts(id) ON DELETE CASCADE,
    package_version        SMALLINT NOT NULL CHECK (package_version = 1),
    recovery_key_id        TEXT NOT NULL,
    ciphertext_base64      TEXT NOT NULL,
    nonce_base64           TEXT NOT NULL,
    associated_data_hash   TEXT NOT NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    rotated_at             TIMESTAMPTZ,
    disabled_at            TIMESTAMPTZ,
    CONSTRAINT encrypted_recovery_package_bounded CHECK (
        octet_length(recovery_key_id) BETWEEN 1 AND 128
        AND octet_length(ciphertext_base64) BETWEEN 16 AND 1048576
        AND octet_length(nonce_base64) BETWEEN 16 AND 128
        AND octet_length(associated_data_hash) BETWEEN 16 AND 256
    )
);

REVOKE ALL ON TABLE threefa.recovery_channels FROM PUBLIC;
REVOKE ALL ON TABLE threefa.recovery_challenges FROM PUBLIC;
REVOKE ALL ON TABLE threefa.encrypted_recovery_packages FROM PUBLIC;
