-- Canonical Postgres schema source for billing-server-rs.
--
-- This file is the desired-state contract for the service's OWN database
-- (separate from the shared pg-defs RDS contract). It is the consolidated
-- final state of migrations/20260518000001 .. 20260609000016; the frozen
-- historical files remain under migrations/ for audit only.
--
-- Do not apply this file directly to a live database; generate and review a
-- migration with scripts/dpm.sh (dpm — declarative-postgres-migrate) instead.
-- The service never migrates at boot.
--
-- Conventions (from the original init migration):
--   * All money is stored in MINOR units as numeric(38, 0). Never floats.
--   * Every tenant-scoped row carries shard_key for future horizontal
--     partitioning. shard_key is derived from (tenant_id, region) and
--     computed by the application.
--   * Postings table is append-only; UPDATE/DELETE is forbidden by trigger.
--   * Every posting carries idempotency_key + (source, source_event_id) for
--     replay safety.

create extension if not exists pgcrypto;
create extension if not exists citext;

------------------------------------------------------------------------------
-- Enumerated types
--
-- Postgres has no `create type if not exists`; dpm materializes this file on
-- a fresh shadow database and diffs catalogs, so bare `create type` is fine.
-- Label order matters to catalog diffs: values appended over time by
-- `alter type ... add value` (migrations 0008/0010/0011/0013/0014/0016) are
-- listed here in exactly the order they were added.
------------------------------------------------------------------------------

create type account_kind as enum (
  'asset',        -- cash, clearing, onchain
  'liability',    -- accounts_payable, unallocated_cash
  'income',       -- revenue
  'expense',      -- fees, chargebacks
  'receivable'    -- accounts_receivable per customer
);

create type account_normal_side as enum ('debit', 'credit');

create type posting_direction as enum ('debit', 'credit');

create type provider_kind as enum (
  'stripe',
  'paypal',
  'braintree',
  'coinbase_commerce',
  'coinbase_prime',
  'plaid_bank',
  'swift_wire',
  'ach_direct',
  'wise',
  'solana_wallet',
  -- added 2026-05-18 (0008): Coinflow — fiat + crypto (Solana settlement) VASP
  'coinflow',
  -- added 2026-05-20 (0010): card / e-money / cross-border providers
  'revolut',
  'remitly',
  'robinhood',
  'mercury',
  'bridge',
  'gocardless',
  -- added 2026-05-22 (0011): crypto houses
  'fireblocks',
  'circle',
  -- added 2026-06-06 (0013): limited-fit remittance partners
  'moneygram',
  'western_union',
  -- added 2026-06-07 (0014): bank-sponsored Zelle, faster-payment rails,
  -- and the EVM observer
  'us_bank_zelle',
  'jpmorgan_zelle',
  'bofa_cashpro_gdd',
  'modern_treasury',
  'dwolla',
  'ethereum_wallet',
  -- added 2026-06-09 (0016): card-acquiring partners (stub maturity)
  'adyen',
  'square'
);

create type provider_auth_kind as enum (
  'oauth2',
  'api_key',
  'bank_coordinates',
  'wallet_pubkey'
);

create type connection_status as enum (
  'pending',
  'active',
  'token_refresh_failed',
  'revoked',
  'expired'
);

create type break_status as enum (
  'open',
  'acknowledged',
  'auto_resolved',
  'resolved'
);

create type lock_event_kind as enum ('acquire', 'renew', 'release', 'preempt', 'expire');

create type schedule_kind as enum ('cron', 'interval', 'one_shot');

create type job_run_status as enum (
  'pending', 'claimed', 'succeeded', 'failed', 'dead_lettered', 'cancelled'
);

create type notification_channel as enum ('email', 'webhook', 'slack', 'sms');

create type notification_dispatch_status as enum (
  'pending', 'sending', 'sent', 'failed', 'throttled', 'suppressed'
);

------------------------------------------------------------------------------
-- Tenants (the B2B customers of the billing server, e.g. dancingdragons.cc)
------------------------------------------------------------------------------

create table if not exists tenants (
  id              uuid primary key default gen_random_uuid(),
  slug            citext not null unique,
  display_name    text   not null,
  country_code    char(2) not null,
  us_state        char(2),
  base_currency   char(3) not null default 'USD',
  kms_key_id      text not null,
  status          text not null default 'active'
                  check (status in ('active', 'suspended', 'terminated')),
  created_at      timestamptz not null default now(),
  updated_at      timestamptz not null default now()
);

create index if not exists tenants_country_idx on tenants (country_code);

-- O(log n) tenant-by-slug lookup for the API auth middleware (0012). Slugs
-- are already unique; this index simply speeds up the per-request lookup
-- used by per-tenant bearer scoping (see src/api/auth.rs).
create index if not exists tenants_slug_lookup_idx on tenants (slug);

------------------------------------------------------------------------------
-- Tenant API keys (tenants authenticate to us; one tenant can have many keys)
------------------------------------------------------------------------------

create table if not exists tenant_api_keys (
  id              uuid primary key default gen_random_uuid(),
  tenant_id       uuid not null references tenants(id) on delete cascade,
  key_prefix      text not null unique,
  key_hash        bytea not null,
  label           text not null,
  scopes          text[] not null default array['read', 'write'],
  last_used_at    timestamptz,
  revoked_at      timestamptz,
  created_at      timestamptz not null default now()
);

create index if not exists tenant_api_keys_tenant_idx on tenant_api_keys (tenant_id);

------------------------------------------------------------------------------
-- Users (end-users / counterparties of a tenant)
-- A user is uniquely identified by (tenant_id, email).
-- A user can be a customer (we bill them), a vendor (we pay them), or both.
------------------------------------------------------------------------------

create table if not exists users (
  id              uuid primary key default gen_random_uuid(),
  tenant_id       uuid not null references tenants(id) on delete cascade,
  shard_key       bigint not null,
  email           citext not null,
  display_name    text,
  country_code    char(2),
  us_state        char(2),
  is_customer     boolean not null default false,
  is_vendor       boolean not null default false,
  external_refs   jsonb not null default '{}'::jsonb,
  metadata        jsonb not null default '{}'::jsonb,
  created_at      timestamptz not null default now(),
  updated_at      timestamptz not null default now(),
  unique (tenant_id, email)
);

create index if not exists users_shard_idx on users (shard_key);
create index if not exists users_tenant_idx on users (tenant_id);
create index if not exists users_external_refs_gin on users using gin (external_refs);

------------------------------------------------------------------------------
-- Accounts (ledger primitive). Multiple accounts per user.
-- Examples: ar/cus_x, ap/ven_y, clearing/stripe, cash/chase, onchain/sol_wallet
------------------------------------------------------------------------------

create table if not exists accounts (
  id              uuid primary key default gen_random_uuid(),
  tenant_id       uuid not null references tenants(id) on delete restrict,
  shard_key       bigint not null,
  user_id         uuid references users(id) on delete restrict,
  kind            account_kind not null,
  normal_side     account_normal_side not null,
  code            text not null,
  currency        char(3) not null,
  metadata        jsonb not null default '{}'::jsonb,
  created_at      timestamptz not null default now(),
  unique (tenant_id, code, currency)
);

create index if not exists accounts_shard_idx on accounts (shard_key);
create index if not exists accounts_tenant_idx on accounts (tenant_id);
create index if not exists accounts_user_idx on accounts (user_id) where user_id is not null;

------------------------------------------------------------------------------
-- Transactions (header) + postings (rows)
-- A transaction is a set of N>=2 postings that MUST sum to zero per currency.
------------------------------------------------------------------------------

create table if not exists transactions (
  id              uuid primary key default gen_random_uuid(),
  tenant_id       uuid not null references tenants(id) on delete restrict,
  shard_key       bigint not null,
  kind            text not null,
  idempotency_key text not null,
  description     text,
  metadata        jsonb not null default '{}'::jsonb,
  posted_at       timestamptz not null default now(),
  unique (tenant_id, idempotency_key)
);

create index if not exists transactions_shard_idx on transactions (shard_key);
create index if not exists transactions_tenant_posted_idx on transactions (tenant_id, posted_at desc);

-- Postings replay protection is TENANT-scoped (0007): provider event ids are
-- not a safe cross-tenant namespace, especially for bank files and synthetic
-- import ids. The original inline `unique (source, source_event_id,
-- direction, account_id)` from the init migration was dropped and replaced
-- by postings_tenant_source_event_direction_account_uq below.
create table if not exists postings (
  id                bigserial primary key,
  transaction_id    uuid not null references transactions(id) on delete restrict,
  tenant_id         uuid not null references tenants(id) on delete restrict,
  shard_key         bigint not null,
  account_id        uuid not null references accounts(id) on delete restrict,
  direction         posting_direction not null,
  amount_minor      numeric(38, 0) not null check (amount_minor > 0),
  currency          char(3) not null,
  source            text not null,
  source_event_id   text not null,
  posted_at         timestamptz not null default now(),
  metadata          jsonb not null default '{}'::jsonb
);

create index if not exists postings_shard_idx on postings (shard_key);
create index if not exists postings_tx_idx on postings (transaction_id);
create index if not exists postings_account_idx on postings (account_id, posted_at desc);
create index if not exists postings_tenant_posted_idx on postings (tenant_id, posted_at desc);

-- Tenant-scoped provider event idempotency (0007).
create unique index if not exists postings_tenant_source_event_direction_account_uq
  on postings (tenant_id, source, source_event_id, direction, account_id);

-- Append-only enforcement
create or replace function postings_immutable()
returns trigger
language plpgsql
-- Body text is kept byte-identical to the migration that created it. Postgres
-- stores `prosrc` verbatim, so re-styling the body would read as schema drift to
-- dpm and provoke a CREATE OR REPLACE on every diff.
as $$
BEGIN
    RAISE EXCEPTION 'postings are append-only; UPDATE/DELETE forbidden';
END;
$$;

drop trigger if exists postings_no_update on postings;
create trigger postings_no_update before update on postings
  for each row execute function postings_immutable();

drop trigger if exists postings_no_delete on postings;
create trigger postings_no_delete before delete on postings
  for each row execute function postings_immutable();

-- Per-transaction zero-sum invariant, checked at COMMIT time.
create or replace function transactions_must_balance()
returns trigger
language plpgsql
-- Body text is kept byte-identical to the migration that created it (see the
-- note on postings_immutable above).
as $$
DECLARE
    bad RECORD;
BEGIN
    FOR bad IN
        SELECT t.id, p.currency,
               SUM(CASE WHEN p.direction = 'debit' THEN p.amount_minor ELSE -p.amount_minor END) AS net
        FROM transactions t
        JOIN postings p ON p.transaction_id = t.id
        WHERE t.id = NEW.id
        GROUP BY t.id, p.currency
        HAVING SUM(CASE WHEN p.direction = 'debit' THEN p.amount_minor ELSE -p.amount_minor END) <> 0
    LOOP
        RAISE EXCEPTION 'transaction % is not balanced in currency %: net=%',
                        bad.id, bad.currency, bad.net;
    END LOOP;
    RETURN NEW;
END;
$$;

drop trigger if exists transactions_balance_check on transactions;
create constraint trigger transactions_balance_check
  after insert on transactions
  deferrable initially deferred
  for each row execute function transactions_must_balance();

------------------------------------------------------------------------------
-- Provider connections & OAuth state
--
-- Tenants connect their payment provider accounts via OAuth (where supported)
-- or by submitting API keys / bank coordinates (otherwise). The raw secret
-- material is sealed with an AES-GCM key wrapped by the tenant's KMS data key
-- and stored as a JSONB blob alongside the wrapping context.
------------------------------------------------------------------------------

create table if not exists provider_connections (
  id                  uuid primary key default gen_random_uuid(),
  tenant_id           uuid not null references tenants(id) on delete cascade,
  shard_key           bigint not null,
  provider            provider_kind not null,
  auth_kind           provider_auth_kind not null,
  external_account_id text,
  display_label       text not null,
  status              connection_status not null default 'pending',

  -- Encrypted credential envelope. Plaintext shape is provider-specific.
  -- AES-256-GCM ciphertext (base64), nonce (base64), and AAD includes
  -- tenant_id + provider so a wrap from tenant A cannot be replayed for B.
  sealed_credential   jsonb,
  kms_key_version     int not null default 1,

  -- OAuth-specific metadata kept in cleartext for operational queries.
  scopes              text[] not null default array[]::text[],
  expires_at          timestamptz,
  refreshed_at        timestamptz,

  last_sync_at        timestamptz,
  last_sync_cursor    text,
  last_error          text,

  metadata            jsonb not null default '{}'::jsonb,
  created_at          timestamptz not null default now(),
  updated_at          timestamptz not null default now(),

  -- A tenant may have multiple connections per provider (e.g. 10 banks via
  -- plaid_bank), so uniqueness is by (tenant, provider, external_account_id)
  -- and only when external_account_id is known.
  unique (tenant_id, provider, external_account_id)
);

create index if not exists provider_connections_tenant_idx on provider_connections (tenant_id);
create index if not exists provider_connections_shard_idx on provider_connections (shard_key);
create index if not exists provider_connections_status_idx on provider_connections (status)
  where status in ('token_refresh_failed', 'expired');

-- Connection sync state and scheduler lookup support (0007).
create index if not exists provider_connections_due_sync_idx
  on provider_connections (tenant_id, provider, last_sync_at)
  where status = 'active';

create index if not exists provider_connections_cursor_idx
  on provider_connections (id, last_sync_cursor)
  where status = 'active';

-- Single-active-connection per (provider, external_account_id) (0012).
-- Without this, `connections.find_active_by_external_account` returned the
-- most-recently-updated row when two tenants registered the same external
-- account id (a Stripe `acct_...`, Plaid `item_id`, etc), so a webhook could
-- be misattributed to the wrong tenant. Scoped to ACTIVE rows only because
-- revoked/expired connections frequently leave stale external_account_id
-- values around.
create unique index if not exists provider_connections_active_external_unique_idx
  on provider_connections (provider, external_account_id)
  where status = 'active'::connection_status
    and external_account_id is not null;

------------------------------------------------------------------------------
-- OAuth state (anti-CSRF nonce store for the OAuth handshake)
------------------------------------------------------------------------------

create table if not exists oauth_states (
  state           text primary key,
  tenant_id       uuid not null references tenants(id) on delete cascade,
  provider        provider_kind not null,
  return_to       text,
  pkce_verifier   text,
  expires_at      timestamptz not null,
  created_at      timestamptz not null default now()
);

create index if not exists oauth_states_expires_idx on oauth_states (expires_at);

------------------------------------------------------------------------------
-- Raw webhook events (kept for replay + audit)
--
-- Hardening history folded in:
--   * 0009 added non-secret verification audit columns (payload_sha256,
--     verification_error, external_account_id) so operators can distinguish
--     "bad JSON", "unknown connection", and "signature failed" without
--     storing signature headers or other bearer material.
--   * 0015 moved payload storage to `payload_sealed` — a SealedEnvelope
--     ({ciphertext_b64, nonce_b64, aad_tag, version}) encrypted with the
--     per-deployment AES-256-GCM master key (BILLING_MASTER_SEAL_KEY, see
--     src/crypto.rs Sealer). The AAD binds each row to its provider, so a
--     sealed blob can't be silently swapped between providers. The plaintext
--     `payload` column is retained but nullable; new rows write only
--     `payload_sealed`.
------------------------------------------------------------------------------

create table if not exists webhook_events (
  id                  bigserial primary key,
  connection_id       uuid references provider_connections(id) on delete set null,
  tenant_id           uuid references tenants(id) on delete cascade,
  provider            provider_kind not null,
  external_event_id   text not null,
  event_type          text not null,
  payload             jsonb,
  signature_ok        boolean not null,
  processed_at        timestamptz,
  process_error       text,
  received_at         timestamptz not null default now(),
  payload_sha256      text,
  verification_error  text,
  external_account_id text,
  payload_sealed      jsonb,
  unique (provider, external_event_id)
);

create index if not exists webhook_events_tenant_idx on webhook_events (tenant_id, received_at desc);
create index if not exists webhook_events_unprocessed_idx on webhook_events (received_at)
  where processed_at is null;

create index if not exists webhook_events_signature_idx
  on webhook_events (provider, signature_ok, received_at desc);

create index if not exists webhook_events_external_account_idx
  on webhook_events (provider, external_account_id, received_at desc)
  where external_account_id is not null;

------------------------------------------------------------------------------
-- Reconciliation breaks
------------------------------------------------------------------------------

create table if not exists reconciliation_breaks (
  id                  bigserial primary key,
  tenant_id           uuid not null references tenants(id) on delete cascade,
  shard_key           bigint not null,
  provider            provider_kind not null,
  connection_id       uuid references provider_connections(id) on delete set null,
  break_type          text not null,
  external_ref        text,
  transaction_id      uuid references transactions(id) on delete set null,
  expected_minor      numeric(38, 0),
  actual_minor        numeric(38, 0),
  currency            char(3),
  status              break_status not null default 'open',
  notes               text,
  detected_at         timestamptz not null default now(),
  resolved_at         timestamptz,
  metadata            jsonb not null default '{}'::jsonb
);

create index if not exists recon_breaks_tenant_open_idx on reconciliation_breaks (tenant_id, status)
  where status = 'open';
create index if not exists recon_breaks_shard_idx on reconciliation_breaks (shard_key);

-- Idempotent break insertion (0011): without this, each retried sync created
-- a new `open` row for the same (provider, connection_id, external_ref) — a
-- single Plaid modified-transaction event could leave dozens of duplicate
-- open breaks. Partial over the open population only, so once a break is
-- acknowledged/resolved the same external_ref can open a fresh break later.
create unique index if not exists recon_breaks_open_unique_idx
  on reconciliation_breaks (provider, connection_id, break_type, external_ref)
  where status = 'open' and external_ref is not null;

------------------------------------------------------------------------------
-- On-chain anchors. We periodically compute a Merkle root over a range of
-- postings and publish it to Solana (via a memo). The signature + slot let
-- anyone independently verify that a posting existed at a known point in time.
------------------------------------------------------------------------------

create table if not exists anchors (
  id                  bigserial primary key,
  tenant_id           uuid not null references tenants(id) on delete cascade,
  shard_key           bigint not null,
  from_posting_id     bigint not null,
  to_posting_id       bigint not null,
  posting_count       bigint not null,
  merkle_root         bytea not null,
  chain               text not null default 'solana',
  tx_signature        text,
  slot                bigint,
  finalized_at        timestamptz,
  submitted_at        timestamptz not null default now(),
  unique (tenant_id, from_posting_id, to_posting_id)
);

create index if not exists anchors_tenant_idx on anchors (tenant_id, submitted_at desc);
create index if not exists anchors_unfinalized_idx on anchors (submitted_at)
  where finalized_at is null;

------------------------------------------------------------------------------
-- Tenant-scoped leases (the lock primitive)
--
-- Leases (not strict mutexes) — every acquire has a TTL so a crashed client
-- cannot hold a lock forever. Callers receive an opaque `lease_token` UUID;
-- renew and release require presenting that token so a third party that
-- merely knows the resource_key cannot steal the lease.
--
-- Backed by Postgres so failover comes "for free" via the same PG HA story
-- used by the ledger. No separate distributed-lock infrastructure required.
------------------------------------------------------------------------------

create table if not exists tenant_locks (
  tenant_id     uuid        not null references tenants(id) on delete cascade,
  shard_key     bigint      not null,
  resource_key  text        not null,
  lease_token   uuid        not null,
  holder        text,
  acquired_at   timestamptz not null,
  expires_at    timestamptz not null,
  metadata      jsonb       not null default '{}'::jsonb,
  primary key (tenant_id, resource_key)
);

create index if not exists tenant_locks_shard_idx on tenant_locks (shard_key);
-- Purge sweeper queries this index.
create index if not exists tenant_locks_expired_idx on tenant_locks (expires_at);

------------------------------------------------------------------------------
-- Durable fencing-token guard for fiducia.cloud grants
--
-- Every fiducia lock/lease yields a fencing token. Those tokens used to be
-- carried back to `release`, reported in API responses, and otherwise ignored:
-- no database write was conditioned on one, so the distributed locks were
-- advisory only. The 60s lease TTL cannot be extended (the SDK has no
-- lock-renewal call), and fiducia's node/brain plane is reached across a
-- cluster boundary — so "lease expired mid-critical-section" is a live
-- failure mode, not a theoretical one. When it happened, a second holder could
-- acquire and BOTH writers would commit, silently.
--
-- This table records the highest fencing token ever accepted per (tenant,
-- fiducia key). A fenced write asserts its token against this row INSIDE the
-- same transaction as the write, so check and commit are atomic and a stale
-- token loses deterministically.
--
-- Why in-transaction: re-asking fiducia "do I still hold this?" over the
-- network is a TOCTOU — the answer is stale the instant it returns and cannot
-- be made atomic with COMMIT. Monotonic tokens in the same transaction are the
-- standard resolution. The pg_advisory_xact_lock taken alongside remains the
-- local backup: it serializes same-database contenders even when fiducia is
-- unreachable, while the fence is what stops a *stale* holder.
------------------------------------------------------------------------------

create table if not exists fiducia_fences (
  tenant_id     uuid        not null references tenants(id) on delete cascade,
  -- The fiducia key this fence guards, e.g. `billing:customer:<tenant>:<id>`.
  fence_key     text        not null,
  -- Highest fencing token ever accepted for this key. Monotonic per key.
  fencing_token bigint      not null check (fencing_token > 0),
  -- Holder that last advanced the fence, so "the same holder re-asserting its
  -- own term" is distinguishable from "a different holder replaying a token".
  holder        text,
  observed_at   timestamptz not null default now(),
  primary key (tenant_id, fence_key)
);

-- Operational visibility and stale-fence cleanup.
create index if not exists fiducia_fences_observed_idx on fiducia_fences (observed_at);

-- Audit trail of every acquire/renew/release. Append-only, retained 90 days
-- by a background job (not yet written). This is the SOC 2 control surface
-- for the lock feature.
create table if not exists tenant_lock_events (
  id            bigserial primary key,
  tenant_id     uuid        not null references tenants(id) on delete cascade,
  shard_key     bigint      not null,
  resource_key  text        not null,
  lease_token   uuid,
  kind          lock_event_kind not null,
  holder        text,
  actor         text,                   -- API caller (e.g. "tenant-api-key:tak_abc")
  ttl_seconds   int,
  occurred_at   timestamptz not null default now(),
  metadata      jsonb       not null default '{}'::jsonb
);

create index if not exists tenant_lock_events_tenant_idx
  on tenant_lock_events (tenant_id, resource_key, occurred_at desc);

------------------------------------------------------------------------------
-- Durable scheduler (the "bulletproof cron")
--
-- Pattern: pg-boss / Sidekiq-PG / River. The runner loop does
--    select ... for update skip locked
-- to atomically claim due jobs, guaranteeing exactly-one execution per due
-- tick across N pods without any external coordination service.
--
-- Every run is recorded in job_runs (durable history). Failures are retried
-- with exponential backoff; after max_attempts a row is copied into
-- dead_letter_jobs so it surfaces on the breaks/ops dashboard.
------------------------------------------------------------------------------

-- Tenant scope is optional so system jobs (lock sweeper, anchor sweeper)
-- can live in the same table as tenant jobs.
create table if not exists scheduled_jobs (
  id                  uuid primary key default gen_random_uuid(),
  tenant_id           uuid references tenants(id) on delete cascade,
  shard_key           bigint not null default 0,
  kind                text not null,              -- e.g. "system.lock_sweeper", "tenant.payroll_run"
  name                text not null,
  schedule_kind       schedule_kind not null,
  cron_expr           text,                       -- when schedule_kind = 'cron'
  interval_seconds    int,                        -- when schedule_kind = 'interval'
  one_shot_at         timestamptz,                -- when schedule_kind = 'one_shot'
  timezone            text not null default 'UTC',
  payload             jsonb not null default '{}'::jsonb,
  enabled             boolean not null default true,
  max_attempts        int not null default 5,
  retry_backoff_secs  int not null default 30,    -- base for exponential backoff
  timeout_seconds     int not null default 300,
  next_run_at         timestamptz not null default now(),
  last_run_at         timestamptz,
  created_at          timestamptz not null default now(),
  updated_at          timestamptz not null default now(),
  -- One named definition per tenant per kind+name pair.
  unique nulls not distinct (tenant_id, kind, name)
);

create index if not exists scheduled_jobs_due_idx
  on scheduled_jobs (enabled, next_run_at)
  where enabled = true;
create index if not exists scheduled_jobs_tenant_idx on scheduled_jobs (tenant_id);

create table if not exists job_runs (
  id                  bigserial primary key,
  job_id              uuid not null references scheduled_jobs(id) on delete cascade,
  tenant_id           uuid references tenants(id) on delete cascade,
  shard_key           bigint not null default 0,
  attempt             int not null default 1,
  status              job_run_status not null,
  scheduled_for       timestamptz not null,
  claimed_at          timestamptz,
  claimed_by          text,                       -- pod / worker id
  finished_at         timestamptz,
  duration_ms         int,
  output              jsonb,
  error               text,
  idempotency_key     text not null,
  unique (job_id, idempotency_key)
);

create index if not exists job_runs_job_idx       on job_runs (job_id, scheduled_for desc);
create index if not exists job_runs_tenant_idx    on job_runs (tenant_id, finished_at desc);
create index if not exists job_runs_status_idx    on job_runs (status) where status in ('pending', 'claimed');

create table if not exists dead_letter_jobs (
  id                  bigserial primary key,
  job_id              uuid not null references scheduled_jobs(id) on delete cascade,
  tenant_id           uuid references tenants(id) on delete cascade,
  last_run_id         bigint references job_runs(id) on delete set null,
  final_attempt       int not null,
  error               text,
  occurred_at         timestamptz not null default now(),
  acknowledged_at     timestamptz
);

create index if not exists dead_letter_jobs_unack_idx
  on dead_letter_jobs (occurred_at desc)
  where acknowledged_at is null;

------------------------------------------------------------------------------
-- Notifications
--
-- A "rule" says: when condition X is true for entity Y in tenant T, send a
-- message via channel Z to target W. Conditions are evaluated by a scheduled
-- job (notifications.evaluate_rules) that runs every N minutes per tenant.
--
-- A "dispatch" is the record of one outbound send. Throttling and dedupe
-- happen against this table (e.g. don't send more than 1 "overdue" notice
-- per customer per day).
------------------------------------------------------------------------------

create table if not exists notification_rules (
  id              uuid primary key default gen_random_uuid(),
  tenant_id       uuid not null references tenants(id) on delete cascade,
  shard_key       bigint not null,
  kind            text not null,
  -- e.g. "balance_negative", "payment_overdue", "payment_received",
  --      "reconciliation_break_opened", "lease_held_too_long"
  name            text not null,
  params          jsonb not null default '{}'::jsonb,
  -- Channel + target ("alice@example.com" / "https://.../webhook" / "#billing-alerts")
  channel         notification_channel not null,
  target          text not null,
  -- Per-channel auth/signing material, sealed in the same envelope shape
  -- as provider credentials. Optional; webhook channel uses it for HMAC,
  -- email channel uses it for provider api key, etc.
  sealed_credential jsonb,
  template_id     text,                       -- opaque ref to a template store; defaults baked in
  throttle_per_day int not null default 1,    -- max dispatches per (rule, target_resource, day)
  enabled         boolean not null default true,
  created_at      timestamptz not null default now(),
  updated_at      timestamptz not null default now(),
  unique (tenant_id, kind, name)
);

create index if not exists notification_rules_tenant_idx on notification_rules (tenant_id);
create index if not exists notification_rules_shard_idx on notification_rules (shard_key);

create table if not exists notification_dispatches (
  id                  bigserial primary key,
  rule_id             uuid not null references notification_rules(id) on delete cascade,
  tenant_id           uuid not null references tenants(id) on delete cascade,
  shard_key           bigint not null,
  -- The thing the dispatch is about, e.g. user_id or invoice_id.
  target_resource     text,
  channel             notification_channel not null,
  target              text not null,
  payload             jsonb not null,
  status              notification_dispatch_status not null default 'pending',
  provider_message_id text,
  error               text,
  sent_at             timestamptz,
  created_at          timestamptz not null default now()
);

create index if not exists notification_dispatches_tenant_idx
  on notification_dispatches (tenant_id, created_at desc);
create index if not exists notification_dispatches_rule_idx
  on notification_dispatches (rule_id, created_at desc);

-- Throttler lookup: one rule/resource over a UTC day range.
create index if not exists notification_dispatches_day_idx
  on notification_dispatches (rule_id, target_resource, created_at desc)
  where status in ('sent', 'pending', 'sending');

------------------------------------------------------------------------------
-- Distributed provider request budgets (0007)
--
-- Provider sync runs on many pods and many tenants; keeping the shared
-- concurrency and idempotency rules in Postgres lets workers fail and
-- recover without double-posting or accidentally stampeding providers.
------------------------------------------------------------------------------

create table if not exists provider_rate_limit_buckets (
  tenant_id        uuid not null references tenants(id) on delete cascade,
  provider         provider_kind not null,
  window_start     timestamptz not null,
  window_seconds   int not null check (window_seconds > 0),
  request_limit    int not null check (request_limit > 0),
  requests_used    int not null default 0 check (requests_used >= 0),
  updated_at       timestamptz not null default now(),
  primary key (tenant_id, provider, window_start, window_seconds)
);

create index if not exists provider_rate_limit_buckets_gc_idx
  on provider_rate_limit_buckets (window_start);

------------------------------------------------------------------------------
-- Provider balance snapshots (0008)
--
-- A stable place to record per-merchant "wallet balance" snapshots (e.g.
-- Coinflow's dashboard shows a per-merchant wallet balance with APY accrual).
-- Reserved for the wallet-balance reconciler.
------------------------------------------------------------------------------

create table if not exists provider_balance_snapshots (
  id              bigserial primary key,
  tenant_id       uuid not null references tenants(id) on delete cascade,
  shard_key       bigint not null,
  connection_id   uuid not null references provider_connections(id) on delete cascade,
  currency        char(3) not null,
  available_minor numeric(38, 0) not null,
  pending_minor   numeric(38, 0) not null default 0,
  apy_bps         int,
  snapshot_at     timestamptz not null default now(),
  raw             jsonb not null default '{}'::jsonb
);

create index if not exists pbs_connection_idx
  on provider_balance_snapshots (connection_id, snapshot_at desc);
