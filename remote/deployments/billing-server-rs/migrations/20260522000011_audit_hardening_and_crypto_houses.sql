-- billing-server-rs :: audit hardening + 2 new crypto-house providers
--
-- (1) Make reconciliation_breaks insertion idempotent.
--
-- Before: each retried sync would create a new `open` recon break row
-- for the same (provider, connection_id, external_ref) — so a single
-- Plaid modified-transaction event could leave dozens of duplicate
-- "open" breaks in the dashboard.
--
-- We add a partial UNIQUE index over the open population only so that
-- once a break is acknowledged/resolved, the same external_ref can
-- generate a fresh break later.

CREATE UNIQUE INDEX IF NOT EXISTS recon_breaks_open_unique_idx
    ON reconciliation_breaks (provider, connection_id, break_type, external_ref)
    WHERE status = 'open' AND external_ref IS NOT NULL;

-- (2) Add `fireblocks` + `circle` to the provider_kind enum.
--
-- Fireblocks: institutional MPC custody / treasury (used by basically
-- every well-funded crypto company). API key + JWT-signed-request auth,
-- webhooks signed with RSA-SHA512 over the body.
--
-- Circle Mint: USDC issuer; every other crypto integration we have
-- settles in USDC, so this closes the loop with a direct mint-side view.

ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'fireblocks';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'circle';
