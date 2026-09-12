-- billing-server-rs :: add limited-fit remittance providers.
--
-- MoneyGram exposes documented partner transfer/status APIs, while Western
-- Union partner access is enrollment-gated and mTLS-based. Both are added as
-- limited-fit provider kinds so tenants can attach credentials and operators
-- can test typed partner DTOs without enabling automatic ledger sync.

ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'moneygram';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'western_union';
