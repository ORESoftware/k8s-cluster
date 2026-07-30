-- Declarative Postgres contract for durable multi-channel communications.
--
-- NAMESPACE: every owned object lives in the `communications` Postgres schema.
-- This contract is shared by the push worker, the email/SMS contact worker, and
-- future postal-mail adapters. It deliberately keeps provider credentials out of
-- Postgres and stores encrypted recipient capabilities rather than plaintext
-- email addresses, phone numbers, postal addresses, or push tokens.
--
-- Migrations are declarative via dpm (declarative-postgres-migrate). Never apply
-- this file directly to a live database and never migrate at application boot.
-- Runtime services use hand-written SeaORM entities; this file is authoritative.
--
-- Supabase/PostgREST RLS policies and owner-safe projections are defined in
-- supabase.sql so the portable schema still converges on ordinary Postgres 17.

create schema if not exists communications;

-- A deliverable destination owned by one verified shared-auth/Supabase user.
-- `target_ciphertext` is application-encrypted. `target_fingerprint` is a keyed
-- or domain-separated SHA-256 hex digest used for dedupe and audit correlation.
create table if not exists communications.endpoints (
  id uuid primary key default gen_random_uuid(),
  tenant_id text not null,
  application_id text not null,
  shared_user_id text not null,
  supabase_user_id uuid,
  installation_id text,
  channel text not null,
  provider text not null,
  provider_environment text not null default 'production',
  target_ciphertext bytea not null,
  target_nonce bytea not null,
  target_key_id text not null,
  target_fingerprint text not null,
  target_metadata jsonb not null default '{}'::jsonb,
  consent_state text not null default 'pending',
  consent_source text,
  consent_version text,
  verified_at timestamptz,
  last_seen_at timestamptz,
  last_success_at timestamptz,
  last_failure_at timestamptz,
  last_provider_code text,
  status text not null default 'active',
  disabled_at timestamptz,
  revoked_at timestamptz,
  replaced_by_endpoint_id uuid references communications.endpoints (id),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint endpoints_channel_chk
    check (channel in ('push', 'email', 'sms', 'postal')),
  constraint endpoints_provider_chk
    check (provider ~ '^[a-z][a-z0-9_-]{1,63}$'),
  constraint endpoints_environment_chk
    check (provider_environment in ('production', 'sandbox', 'test')),
  constraint endpoints_target_fingerprint_chk
    check (target_fingerprint ~ '^[0-9a-f]{64}$'),
  constraint endpoints_target_key_id_chk
    check (length(target_key_id) between 1 and 128),
  constraint endpoints_target_nonce_chk
    check (octet_length(target_nonce) between 12 and 64),
  constraint endpoints_target_ciphertext_chk
    check (octet_length(target_ciphertext) between 1 and 65536),
  constraint endpoints_target_metadata_chk
    check (jsonb_typeof(target_metadata) = 'object'),
  constraint endpoints_consent_state_chk
    check (consent_state in ('pending', 'granted', 'denied', 'withdrawn', 'not_required')),
  constraint endpoints_status_chk
    check (status in ('active', 'disabled', 'revoked', 'replaced')),
  constraint endpoints_disabled_state_chk
    check (status <> 'disabled' or disabled_at is not null),
  constraint endpoints_revoked_state_chk
    check (status <> 'revoked' or revoked_at is not null),
  constraint endpoints_replaced_state_chk
    check (status <> 'replaced' or replaced_by_endpoint_id is not null)
);

create unique index if not exists endpoints_active_target_uniq
  on communications.endpoints
  (tenant_id, application_id, channel, provider, provider_environment, target_fingerprint)
  where status = 'active';

create unique index if not exists endpoints_active_installation_uniq
  on communications.endpoints
  (tenant_id, application_id, installation_id, channel, provider, provider_environment)
  where status = 'active' and installation_id is not null;

create index if not exists endpoints_user_active_idx
  on communications.endpoints
  (tenant_id, application_id, shared_user_id, channel, status, updated_at desc);

create index if not exists endpoints_supabase_user_idx
  on communications.endpoints (supabase_user_id, status, updated_at desc)
  where supabase_user_id is not null;

create index if not exists endpoints_provider_failure_idx
  on communications.endpoints (provider, last_provider_code, last_failure_at desc)
  where last_failure_at is not null;

-- User-selected channel policy. The service validates channel_order and
-- purpose-specific rules; the JSON document must not contain recipient targets.
create table if not exists communications.preferences (
  id uuid primary key default gen_random_uuid(),
  tenant_id text not null,
  application_id text not null,
  shared_user_id text not null,
  supabase_user_id uuid,
  purpose text not null,
  enabled boolean not null default true,
  channel_order jsonb not null default '["push","email"]'::jsonb,
  quiet_hours_start time,
  quiet_hours_end time,
  timezone text not null default 'UTC',
  locale text,
  sms_opt_in_verified_at timestamptz,
  postal_opt_in_verified_at timestamptz,
  consent_version text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint preferences_purpose_chk
    check (purpose ~ '^[a-z][a-z0-9_.-]{0,127}$'),
  constraint preferences_channel_order_chk
    check (jsonb_typeof(channel_order) = 'array'),
  constraint preferences_quiet_hours_chk
    check ((quiet_hours_start is null) = (quiet_hours_end is null)),
  constraint preferences_user_purpose_uniq
    unique (tenant_id, application_id, shared_user_id, purpose)
);

create index if not exists preferences_supabase_user_idx
  on communications.preferences (supabase_user_id, application_id, purpose)
  where supabase_user_id is not null;

-- One logical communication intent. Content is encrypted before persistence.
-- The idempotency key is scoped to tenant/application and prevents duplicate
-- user-visible communications when producers retry or workers crash.
create table if not exists communications.jobs (
  id uuid primary key default gen_random_uuid(),
  tenant_id text not null,
  application_id text not null,
  shared_user_id text not null,
  supabase_user_id uuid,
  purpose text not null,
  idempotency_key text not null,
  contract_version text not null default 'communications.v1',
  template_id text,
  locale text,
  content_ciphertext bytea not null,
  content_nonce bytea not null,
  content_key_id text not null,
  content_fingerprint text not null,
  delivery_policy jsonb not null default '{}'::jsonb,
  state text not null default 'pending',
  priority integer not null default 0,
  scheduled_at timestamptz not null default now(),
  expires_at timestamptz,
  claimed_by text,
  lease_expires_at timestamptz,
  attempt_count integer not null default 0,
  max_attempts integer not null default 8,
  next_attempt_at timestamptz not null default now(),
  last_error_code text,
  last_safe_detail text,
  traceparent text,
  correlation_id text,
  requested_by_shared_user_id text,
  requested_by_authority text,
  accepted_at timestamptz,
  delivered_at timestamptz,
  failed_at timestamptz,
  cancelled_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint jobs_idempotency_uniq
    unique (tenant_id, application_id, idempotency_key),
  constraint jobs_purpose_chk
    check (purpose ~ '^[a-z][a-z0-9_.-]{0,127}$'),
  constraint jobs_contract_version_chk
    check (contract_version ~ '^[a-z][a-z0-9_.-]{0,63}$'),
  constraint jobs_content_fingerprint_chk
    check (content_fingerprint ~ '^[0-9a-f]{64}$'),
  constraint jobs_content_key_id_chk
    check (length(content_key_id) between 1 and 128),
  constraint jobs_content_nonce_chk
    check (octet_length(content_nonce) between 12 and 64),
  constraint jobs_content_ciphertext_chk
    check (octet_length(content_ciphertext) between 1 and 2097152),
  constraint jobs_delivery_policy_chk
    check (jsonb_typeof(delivery_policy) = 'object'),
  constraint jobs_state_chk
    check (state in (
      'pending', 'scheduled', 'leased', 'sending', 'accepted',
      'delivered', 'failed', 'dead_lettered', 'cancelled', 'expired'
    )),
  constraint jobs_priority_chk check (priority between -1000 and 1000),
  constraint jobs_attempt_count_chk check (attempt_count >= 0),
  constraint jobs_max_attempts_chk check (max_attempts between 1 and 100),
  constraint jobs_expiry_chk check (expires_at is null or expires_at > created_at),
  constraint jobs_lease_chk check (
    state not in ('leased', 'sending')
    or (claimed_by is not null and lease_expires_at is not null)
  ),
  constraint jobs_terminal_timestamp_chk check (
    (state <> 'delivered' or delivered_at is not null)
    and (state not in ('failed', 'dead_lettered') or failed_at is not null)
    and (state <> 'cancelled' or cancelled_at is not null)
  )
);

create index if not exists jobs_claim_idx
  on communications.jobs
  (state, next_attempt_at, scheduled_at, priority desc, created_at asc)
  where state in ('pending', 'scheduled');

create index if not exists jobs_expired_lease_idx
  on communications.jobs (lease_expires_at)
  where state in ('leased', 'sending');

create index if not exists jobs_user_history_idx
  on communications.jobs
  (tenant_id, application_id, shared_user_id, created_at desc);

create index if not exists jobs_supabase_history_idx
  on communications.jobs (supabase_user_id, created_at desc)
  where supabase_user_id is not null;

-- A concrete provider attempt for one logical job. Provider message IDs are
-- correlation identifiers, not credentials, and may arrive after initial accept.
create table if not exists communications.attempts (
  id uuid primary key default gen_random_uuid(),
  job_id uuid not null references communications.jobs (id) on delete cascade,
  endpoint_id uuid references communications.endpoints (id),
  attempt_number integer not null,
  channel text not null,
  provider text not null,
  provider_environment text not null default 'production',
  provider_message_id text,
  request_fingerprint text not null,
  state text not null default 'pending',
  outcome_class text,
  provider_code text,
  retry_after_at timestamptz,
  safe_detail text,
  latency_ms bigint,
  traceparent text,
  started_at timestamptz not null default now(),
  accepted_at timestamptz,
  completed_at timestamptz,
  last_receipt_occurred_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint attempts_job_number_uniq unique (job_id, attempt_number),
  constraint attempts_channel_chk
    check (channel in ('push', 'email', 'sms', 'postal')),
  constraint attempts_provider_chk
    check (provider ~ '^[a-z][a-z0-9_-]{1,63}$'),
  constraint attempts_environment_chk
    check (provider_environment in ('production', 'sandbox', 'test')),
  constraint attempts_request_fingerprint_chk
    check (request_fingerprint ~ '^[0-9a-f]{64}$'),
  constraint attempts_state_chk
    check (state in (
      'pending', 'sending', 'accepted', 'delivered', 'failed',
      'suppressed', 'cancelled', 'unknown'
    )),
  constraint attempts_outcome_class_chk check (
    outcome_class is null or outcome_class in (
      'accepted', 'invalid_target', 'invalid_payload', 'throttled',
      'transient_provider_failure', 'permanent_provider_failure',
      'internal_failure', 'delivered', 'suppressed'
    )
  ),
  constraint attempts_number_chk check (attempt_number between 1 and 100),
  constraint attempts_latency_chk check (latency_ms is null or latency_ms >= 0)
);

create unique index if not exists attempts_provider_message_uniq
  on communications.attempts (provider, provider_message_id)
  where provider_message_id is not null;

create index if not exists attempts_job_idx
  on communications.attempts (job_id, attempt_number desc);

create index if not exists attempts_provider_state_idx
  on communications.attempts (provider, state, updated_at desc);

-- Request-level provider webhook audit. Raw webhook bodies are never retained;
-- the digest provides replay/forensic correlation without storing recipient PII.
create table if not exists communications.webhook_requests (
  id uuid primary key default gen_random_uuid(),
  provider text not null,
  provider_request_id text not null,
  payload_sha256 text not null,
  signature_verified boolean not null,
  signature_timestamp timestamptz,
  event_count integer not null default 0,
  state text not null default 'received',
  safe_error text,
  received_at timestamptz not null default now(),
  processed_at timestamptz,
  constraint webhook_requests_provider_request_uniq
    unique (provider, provider_request_id),
  constraint webhook_requests_provider_chk
    check (provider ~ '^[a-z][a-z0-9_-]{1,63}$'),
  constraint webhook_requests_payload_sha_chk
    check (payload_sha256 ~ '^[0-9a-f]{64}$'),
  constraint webhook_requests_event_count_chk check (event_count >= 0),
  constraint webhook_requests_state_chk
    check (state in ('received', 'processed', 'rejected', 'failed')),
  constraint webhook_requests_processed_chk
    check (state <> 'processed' or processed_at is not null)
);

create index if not exists webhook_requests_received_idx
  on communications.webhook_requests (provider, received_at desc);

-- Append-only provider events. SendGrid sg_event_id, Twilio callback hashes,
-- Expo receipt IDs, push-provider responses, and postal tracking events all map
-- here. Events can arrive out of order; application projection logic must use
-- provider-specific transition rules rather than received_at ordering alone.
create table if not exists communications.receipts (
  id uuid primary key default gen_random_uuid(),
  webhook_request_id uuid references communications.webhook_requests (id) on delete set null,
  attempt_id uuid references communications.attempts (id) on delete set null,
  provider text not null,
  provider_event_id text not null,
  provider_message_id text,
  event_type text not null,
  normalized_state text not null,
  terminal boolean not null default false,
  payload_sha256 text not null,
  signature_verified boolean not null,
  occurred_at timestamptz,
  received_at timestamptz not null default now(),
  sanitized_metadata jsonb not null default '{}'::jsonb,
  constraint receipts_provider_event_uniq
    unique (provider, provider_event_id),
  constraint receipts_provider_chk
    check (provider ~ '^[a-z][a-z0-9_-]{1,63}$'),
  constraint receipts_event_type_chk
    check (event_type ~ '^[a-z][a-z0-9_.-]{0,127}$'),
  constraint receipts_state_chk
    check (normalized_state in (
      'queued', 'processed', 'accepted', 'sent', 'delivered', 'deferred',
      'opened', 'clicked', 'read', 'bounced', 'dropped', 'failed',
      'undelivered', 'complained', 'unsubscribed', 'returned', 'unknown'
    )),
  constraint receipts_payload_sha_chk
    check (payload_sha256 ~ '^[0-9a-f]{64}$'),
  constraint receipts_metadata_chk
    check (jsonb_typeof(sanitized_metadata) = 'object')
);

create index if not exists receipts_attempt_idx
  on communications.receipts (attempt_id, occurred_at, received_at);

create index if not exists receipts_provider_message_idx
  on communications.receipts (provider, provider_message_id, occurred_at, received_at)
  where provider_message_id is not null;

create index if not exists receipts_unmatched_idx
  on communications.receipts (provider, received_at)
  where attempt_id is null;

-- Cross-channel suppression/opt-out ledger. This is the durable guard against
-- retrying bounced email addresses, STOPped phone numbers, invalid push tokens,
-- or postal addresses returned as undeliverable.
create table if not exists communications.suppressions (
  id uuid primary key default gen_random_uuid(),
  tenant_id text not null,
  application_id text not null,
  shared_user_id text,
  endpoint_id uuid references communications.endpoints (id) on delete set null,
  channel text not null,
  provider text,
  scope text not null default 'application',
  reason_code text not null,
  source_receipt_id uuid references communications.receipts (id) on delete set null,
  active boolean not null default true,
  starts_at timestamptz not null default now(),
  ends_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint suppressions_channel_chk
    check (channel in ('push', 'email', 'sms', 'postal')),
  constraint suppressions_provider_chk
    check (provider is null or provider ~ '^[a-z][a-z0-9_-]{1,63}$'),
  constraint suppressions_scope_chk
    check (scope in ('endpoint', 'purpose', 'application', 'tenant', 'global')),
  constraint suppressions_reason_chk
    check (reason_code ~ '^[a-z][a-z0-9_.-]{0,127}$'),
  constraint suppressions_time_chk check (ends_at is null or ends_at > starts_at)
);

create unique index if not exists suppressions_active_endpoint_uniq
  on communications.suppressions
  (tenant_id, application_id, endpoint_id, channel, coalesce(provider, ''), reason_code)
  where active and endpoint_id is not null;

create index if not exists suppressions_lookup_idx
  on communications.suppressions
  (tenant_id, application_id, shared_user_id, channel, active, starts_at desc);

-- Transactional producer outbox. Applications insert the encrypted event in the
-- same transaction as their business state; workers claim rows with
-- FOR UPDATE SKIP LOCKED and publish a communications job exactly once per
-- deterministic event key under at-least-once execution.
create table if not exists communications.outbox (
  id uuid primary key default gen_random_uuid(),
  tenant_id text not null,
  application_id text not null,
  event_key text not null,
  event_type text not null,
  contract_version text not null default 'communications.intent.v1',
  event_ciphertext bytea not null,
  event_nonce bytea not null,
  event_key_id text not null,
  event_fingerprint text not null,
  state text not null default 'pending',
  available_at timestamptz not null default now(),
  claimed_by text,
  lease_expires_at timestamptz,
  attempt_count integer not null default 0,
  max_attempts integer not null default 20,
  published_job_id uuid references communications.jobs (id) on delete set null,
  last_error_code text,
  last_safe_detail text,
  published_at timestamptz,
  dead_lettered_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint outbox_event_key_uniq
    unique (tenant_id, application_id, event_key),
  constraint outbox_event_type_chk
    check (event_type ~ '^[a-z][a-z0-9_.-]{0,127}$'),
  constraint outbox_contract_version_chk
    check (contract_version ~ '^[a-z][a-z0-9_.-]{0,63}$'),
  constraint outbox_event_fingerprint_chk
    check (event_fingerprint ~ '^[0-9a-f]{64}$'),
  constraint outbox_event_key_id_chk
    check (length(event_key_id) between 1 and 128),
  constraint outbox_event_nonce_chk
    check (octet_length(event_nonce) between 12 and 64),
  constraint outbox_event_ciphertext_chk
    check (octet_length(event_ciphertext) between 1 and 2097152),
  constraint outbox_state_chk
    check (state in ('pending', 'leased', 'published', 'retry', 'dead_lettered', 'cancelled')),
  constraint outbox_attempt_count_chk check (attempt_count >= 0),
  constraint outbox_max_attempts_chk check (max_attempts between 1 and 100),
  constraint outbox_lease_chk check (
    state <> 'leased' or (claimed_by is not null and lease_expires_at is not null)
  ),
  constraint outbox_published_chk check (
    state <> 'published' or (published_at is not null and published_job_id is not null)
  ),
  constraint outbox_dead_lettered_chk check (
    state <> 'dead_lettered' or dead_lettered_at is not null
  )
);

create index if not exists outbox_claim_idx
  on communications.outbox (state, available_at, created_at)
  where state in ('pending', 'retry');

create index if not exists outbox_expired_lease_idx
  on communications.outbox (lease_expires_at)
  where state = 'leased';
