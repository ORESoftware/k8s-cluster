-- billing-server-rs :: card-acquiring partner providers (Adyen, Square).
--
-- Adds two card-acquiring partners to the provider_kind enum. Both land at
-- "stub" maturity: connection, API-key credential validation, and real
-- webhook-signature verification are wired (Adyen field-concatenation
-- HMAC-SHA256; Square HMAC-SHA256 over notification-url + body), while the
-- programmatic settlement/payout sync surface is intentionally deferred until
-- a tenant contract maps cleanly to postings (mirrors the Model-A posture of
-- the other limited/stub providers).
--
-- ADD VALUE IF NOT EXISTS is idempotent and, on PG12+, runs inside the
-- migration transaction. The new values are only referenced at runtime, so
-- the "can't use a new enum value in the same transaction" restriction does
-- not apply here.

ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'adyen';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'square';
