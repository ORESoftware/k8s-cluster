-- billing-server-rs :: webhook verification hardening
--
-- Keep enough non-secret audit metadata for inbound provider deliveries so
-- operators can distinguish "bad JSON", "unknown connection", and "signature
-- failed" without storing signature headers or other bearer material.

ALTER TABLE webhook_events
    ADD COLUMN IF NOT EXISTS payload_sha256 TEXT,
    ADD COLUMN IF NOT EXISTS verification_error TEXT,
    ADD COLUMN IF NOT EXISTS external_account_id TEXT;

CREATE INDEX IF NOT EXISTS webhook_events_signature_idx
    ON webhook_events (provider, signature_ok, received_at DESC);

CREATE INDEX IF NOT EXISTS webhook_events_external_account_idx
    ON webhook_events (provider, external_account_id, received_at DESC)
    WHERE external_account_id IS NOT NULL;
