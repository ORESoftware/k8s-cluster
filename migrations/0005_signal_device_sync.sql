-- Signal-family encrypted multi-device account sync.
--
-- Zero-knowledge invariant: this schema stores public prekey material, routing
-- metadata, and opaque ciphertext only. Device identity private keys, signed
-- prekey private keys, post-quantum private keys, one-time prekey private keys,
-- Double Ratchet session state, vault keys, OTP seeds, and plaintext mutations
-- must never enter PostgreSQL.

ALTER TABLE threefa.accounts
    ADD COLUMN IF NOT EXISTS signal_device_revision BIGINT NOT NULL DEFAULT 0;

ALTER TABLE threefa.accounts
    DROP CONSTRAINT IF EXISTS accounts_signal_device_revision_nonnegative;
ALTER TABLE threefa.accounts
    ADD CONSTRAINT accounts_signal_device_revision_nonnegative
    CHECK (signal_device_revision >= 0);

-- Composite uniqueness lets mailbox foreign keys prove that both sender and
-- recipient belong to the stated account, instead of relying only on handlers.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conrelid = 'threefa.devices'::regclass
          AND conname = 'devices_account_id_id_unique'
    ) THEN
        ALTER TABLE threefa.devices
            ADD CONSTRAINT devices_account_id_id_unique UNIQUE (account_id, id);
    END IF;
END
$$;

-- One current public bundle per active device. Keys are base64 text internally
-- because the shared pg-defs boundary represents opaque bytes as base64.
CREATE TABLE IF NOT EXISTS threefa.device_prekey_bundles (
    device_id                       UUID PRIMARY KEY
        REFERENCES threefa.devices(id) ON DELETE CASCADE,
    bundle_revision                 BIGINT NOT NULL CHECK (bundle_revision > 0),
    protocol_version                SMALLINT NOT NULL CHECK (protocol_version = 1),
    registration_id                 BIGINT NOT NULL CHECK (registration_id > 0),
    identity_key_base64             TEXT NOT NULL,
    signed_prekey_id                BIGINT NOT NULL CHECK (signed_prekey_id >= 0),
    signed_prekey_base64            TEXT NOT NULL,
    signed_prekey_signature_base64  TEXT NOT NULL,
    pq_signed_prekey_id             BIGINT NOT NULL CHECK (pq_signed_prekey_id >= 0),
    pq_signed_prekey_base64         TEXT NOT NULL,
    pq_signed_prekey_signature_base64 TEXT NOT NULL,
    published_at                    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at                      TIMESTAMPTZ NOT NULL,
    CONSTRAINT device_prekey_bundle_expiry CHECK (
        expires_at > published_at
        AND expires_at <= published_at + INTERVAL '90 days'
    ),
    CONSTRAINT device_prekey_bundle_identity_size CHECK (
        octet_length(identity_key_base64) BETWEEN 40 AND 8192
    ),
    CONSTRAINT device_prekey_bundle_signed_size CHECK (
        octet_length(signed_prekey_base64) BETWEEN 40 AND 8192
        AND octet_length(signed_prekey_signature_base64) BETWEEN 40 AND 1024
    ),
    CONSTRAINT device_prekey_bundle_pq_size CHECK (
        octet_length(pq_signed_prekey_base64) BETWEEN 40 AND 8192
        AND octet_length(pq_signed_prekey_signature_base64) BETWEEN 40 AND 1024
    )
);

-- One-time prekeys are claimed exactly once. The claim transaction must select
-- one unclaimed row FOR UPDATE SKIP LOCKED and set both claim columns before
-- returning the public key to the requesting sibling device.
CREATE TABLE IF NOT EXISTS threefa.device_one_time_prekeys (
    device_id              UUID NOT NULL
        REFERENCES threefa.devices(id) ON DELETE CASCADE,
    prekey_id              BIGINT NOT NULL CHECK (prekey_id >= 0),
    public_key_base64      TEXT NOT NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_at             TIMESTAMPTZ,
    claimed_by_device_id   UUID REFERENCES threefa.devices(id) ON DELETE SET NULL,
    PRIMARY KEY (device_id, prekey_id),
    CONSTRAINT device_one_time_prekey_size CHECK (
        octet_length(public_key_base64) BETWEEN 40 AND 8192
    ),
    CONSTRAINT device_one_time_prekey_claim_is_complete CHECK (
        (claimed_at IS NULL AND claimed_by_device_id IS NULL)
        OR (claimed_at IS NOT NULL AND claimed_by_device_id IS NOT NULL)
    ),
    CONSTRAINT device_one_time_prekey_not_self_claimed CHECK (
        claimed_by_device_id IS NULL OR claimed_by_device_id <> device_id
    )
);

CREATE INDEX IF NOT EXISTS device_one_time_prekeys_available_idx
    ON threefa.device_one_time_prekeys (device_id, prekey_id)
    WHERE claimed_at IS NULL;

-- Opaque, recipient-specific Signal ciphertext. mailbox_seq is a server cursor;
-- envelope_id is the cryptographic replay/idempotency key supplied by the
-- client and is globally unique.
CREATE TABLE IF NOT EXISTS threefa.device_mailbox (
    mailbox_seq           BIGSERIAL UNIQUE,
    envelope_id           UUID PRIMARY KEY,
    account_id            UUID NOT NULL
        REFERENCES threefa.accounts(id) ON DELETE CASCADE,
    sender_device_id      UUID NOT NULL,
    recipient_device_id   UUID NOT NULL,
    protocol_version      SMALLINT NOT NULL CHECK (protocol_version = 1),
    session_id            TEXT NOT NULL,
    message_number        NUMERIC(20, 0) NOT NULL CHECK (
        message_number >= 0
        AND message_number <= 18446744073709551615
    ),
    kind                  TEXT NOT NULL CHECK (
        kind IN ('vault_snapshot', 'vault_mutation', 'vault_key_transfer', 'device_control')
    ),
    client_created_at_ms  BIGINT NOT NULL CHECK (client_created_at_ms >= 0),
    expires_at_ms         BIGINT NOT NULL CHECK (expires_at_ms > client_created_at_ms),
    ciphertext_base64     TEXT NOT NULL,
    ciphertext_size_bytes INTEGER NOT NULL CHECK (
        ciphertext_size_bytes BETWEEN 1 AND 524288
    ),
    queued_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at          TIMESTAMPTZ,
    acknowledged_at       TIMESTAMPTZ,
    CONSTRAINT device_mailbox_sender_not_recipient CHECK (
        sender_device_id <> recipient_device_id
    ),
    CONSTRAINT device_mailbox_session_id_shape CHECK (
        char_length(session_id) BETWEEN 1 AND 128
        AND session_id ~ '^[A-Za-z0-9_.:-]+$'
    ),
    CONSTRAINT device_mailbox_lifetime_bounded CHECK (
        expires_at_ms - client_created_at_ms <= 2592000000
    ),
    CONSTRAINT device_mailbox_ciphertext_base64_bounded CHECK (
        octet_length(ciphertext_base64) BETWEEN 4 AND 699052
    ),
    CONSTRAINT device_mailbox_sender_account_fk
        FOREIGN KEY (account_id, sender_device_id)
        REFERENCES threefa.devices(account_id, id) ON DELETE CASCADE,
    CONSTRAINT device_mailbox_recipient_account_fk
        FOREIGN KEY (account_id, recipient_device_id)
        REFERENCES threefa.devices(account_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS device_mailbox_recipient_cursor_idx
    ON threefa.device_mailbox (recipient_device_id, mailbox_seq)
    WHERE acknowledged_at IS NULL;

CREATE INDEX IF NOT EXISTS device_mailbox_expiry_idx
    ON threefa.device_mailbox (expires_at_ms);

-- Public-key rotations and device-list changes are auditable without recording
-- key material or ciphertext in logs/telemetry.
CREATE TABLE IF NOT EXISTS threefa.device_security_events (
    event_seq             BIGSERIAL PRIMARY KEY,
    account_id            UUID NOT NULL
        REFERENCES threefa.accounts(id) ON DELETE CASCADE,
    actor_device_id       UUID REFERENCES threefa.devices(id) ON DELETE SET NULL,
    subject_device_id     UUID REFERENCES threefa.devices(id) ON DELETE SET NULL,
    device_revision       BIGINT NOT NULL CHECK (device_revision >= 0),
    event_kind            TEXT NOT NULL CHECK (
        event_kind IN (
            'device_enrolled',
            'device_revoked',
            'identity_key_changed',
            'signed_prekey_rotated',
            'prekeys_replenished'
        )
    ),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS device_security_events_account_idx
    ON threefa.device_security_events (account_id, event_seq);

-- The Rust service is the only writer/reader for these tables. Supabase clients
-- must use authenticated RPC/API boundaries rather than direct table access.
REVOKE ALL ON TABLE threefa.device_prekey_bundles FROM PUBLIC;
REVOKE ALL ON TABLE threefa.device_one_time_prekeys FROM PUBLIC;
REVOKE ALL ON TABLE threefa.device_mailbox FROM PUBLIC;
REVOKE ALL ON TABLE threefa.device_security_events FROM PUBLIC;
REVOKE ALL ON SEQUENCE threefa.device_mailbox_mailbox_seq_seq FROM PUBLIC;
REVOKE ALL ON SEQUENCE threefa.device_security_events_event_seq_seq FROM PUBLIC;
