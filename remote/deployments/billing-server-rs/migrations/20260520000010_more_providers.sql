-- billing-server-rs :: add additional payment providers.
--
-- This migration extends provider_kind with six new variants:
--
--   * revolut       — Revolut Business (EU/UK e-money institution). Accounts,
--                     transactions, counterparties, FX. OAuth or API token.
--   * coinbase_exchange (already in enum as coinbase_commerce; we add a
--                     separate path for the institutional Coinbase Exchange
--                     v3 REST API. NOT done in this migration — handled via
--                     coinbase_prime variant.)
--   * remitly       — Consumer remittance. No real B2B API today. Added so
--                     the enum doesn't reject it; sync stays not_implemented.
--   * robinhood     — Brokerage / Robinhood Crypto. Limited API; only useful
--                     as a read-only crypto holdings observer. Honest stub.
--   * mercury       — Mercury banking for startups. Treasury, ACH, wires.
--                     API exists; integration is a separate follow-up.
--   * bridge        — Bridge.xyz (Stripe-owned) stablecoin orchestration.
--                     USDC issuance/redemption with VASP-license leverage.
--   * gocardless    — Direct debit + open banking across UK/EU/AU/US.
--
-- PG12+ allows ALTER TYPE ... ADD VALUE in a transaction provided the value
-- is not used within the same transaction (we only add values here).

ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'revolut';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'remitly';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'robinhood';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'mercury';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'bridge';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'gocardless';
