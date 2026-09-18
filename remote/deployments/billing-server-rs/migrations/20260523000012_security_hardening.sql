-- billing-server-rs :: security & isolation hardening
--
-- Three changes, each documented inline below.

-- (1) Enforce single-active-account per (provider, external_account_id).
--
-- Before: `connections.find_active_by_external_account` returned the
-- most-recently-updated row when two tenants happened to register the
-- same external account ID (a Stripe `acct_...`, Plaid `item_id`, etc).
-- That meant a webhook could be misattributed to the wrong tenant.
--
-- After: a partial unique index makes the misconfiguration impossible
-- to commit in the first place. We scope the constraint to ACTIVE rows
-- only because revoked/expired connections frequently leave stale
-- `external_account_id` values around.
--
-- Migrations of existing rows: the table is new and we have no prod
-- data yet, so we don't need a backfill cleanup step. If we later
-- discover legacy duplicates the migration will fail loudly — that's
-- a feature, not a bug.

CREATE UNIQUE INDEX IF NOT EXISTS provider_connections_active_external_unique_idx
    ON provider_connections (provider, external_account_id)
    WHERE status = 'active'::connection_status
      AND external_account_id IS NOT NULL;

-- (2) Tighten posting source uniqueness to be tenant-scoped (defense in
--     depth, not a correctness fix).
--
-- We already have `postings_unique_per_tenant_source` covering this in
-- migration 0007; this is a NO-OP placeholder so we don't drift the
-- numbering. The previous migration is the load-bearing one.

-- (3) Add an index that lets the API auth middleware look up a tenant
--     by its short slug in O(log n). Slugs are already UNIQUE so this
--     just speeds up the per-request lookup.
--
-- Used by per-tenant bearer scoping when BILLING_API_AUTH_TENANT_TOKENS
-- is configured as a CSV mapping (see src/api/auth.rs).

CREATE INDEX IF NOT EXISTS tenants_slug_lookup_idx
    ON tenants (slug);
