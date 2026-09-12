-- billing-server-rs :: encrypt webhook payloads at rest.
--
-- Inbound provider webhook bodies frequently carry PII and provider-side
-- identifiers. They were previously stored verbatim in
-- `webhook_events.payload` (JSONB, plaintext). This migration moves storage
-- to a sealed envelope encrypted with the same per-deployment AES-256-GCM
-- master key (`BILLING_MASTER_SEAL_KEY`) used for provider credentials
-- (src/crypto.rs `Sealer`), closing the readme follow-up
-- "Webhook payloads stored unencrypted at rest".
--
-- `payload_sealed` holds the serialized SealedEnvelope
-- ({ciphertext_b64, nonce_b64, aad_tag, version}). The AAD binds each row to
-- its provider (and tenant when known), so a sealed blob can't be silently
-- swapped between providers. The plaintext `payload` column is retained but
-- relaxed to NULL: new rows write only `payload_sealed`. Any historical
-- plaintext rows are left untouched (no online re-encrypt here) and can be
-- backfilled by an out-of-band job if required.
--
-- There are no SELECT readers of `payload` in the server today (it is
-- write-only audit storage), so this is a storage-format change with no read
-- path to update.

ALTER TABLE webhook_events
    ADD COLUMN IF NOT EXISTS payload_sealed JSONB;

ALTER TABLE webhook_events
    ALTER COLUMN payload DROP NOT NULL;
