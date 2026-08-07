-- Hosted payment checkout orchestration.
--
-- This table is intentionally provider-neutral at its boundary even though the
-- first implementation uses Stripe Checkout. Signed provider webhooks remain
-- the authoritative money-movement input; these rows correlate a tenant's
-- application intent with the provider session used to collect advance payment.

create table if not exists checkout_sessions (
  id                       uuid primary key default gen_random_uuid(),
  tenant_id                uuid not null references tenants(id) on delete restrict,
  shard_key                bigint not null,
  provider_connection_id   uuid not null references provider_connections(id) on delete restrict,
  idempotency_key_hash     text not null,
  intent_fingerprint       text not null,
  client_reference_id      text not null,
  customer_email_hash      text not null,
  amount_minor             numeric(38, 0) not null check (amount_minor > 0),
  currency                 char(3) not null,
  description              text not null,
  metadata                 jsonb not null default '{}'::jsonb,
  provider_session_id      text,
  checkout_url             text,
  session_status           text not null default 'creating',
  payment_status           text not null default 'unpaid',
  created_at               timestamptz not null default now(),
  updated_at               timestamptz not null default now(),

  constraint checkout_sessions_idempotency_hash_chk check (
    idempotency_key_hash ~ '^sha256:v1:[0-9a-f]{64}$'
  ),
  constraint checkout_sessions_intent_fingerprint_chk check (
    intent_fingerprint ~ '^sha256:v1:[0-9a-f]{64}$'
  ),
  constraint checkout_sessions_customer_email_hash_chk check (
    customer_email_hash ~ '^sha256:v1:[0-9a-f]{64}$'
  ),
  constraint checkout_sessions_client_reference_chk check (
    octet_length(client_reference_id) between 1 and 200
  ),
  constraint checkout_sessions_currency_chk check (
    currency ~ '^[A-Z]{3}$'
  ),
  constraint checkout_sessions_description_chk check (
    octet_length(description) between 1 and 200
  ),
  constraint checkout_sessions_metadata_object_chk check (
    jsonb_typeof(metadata) = 'object'
  ),
  constraint checkout_sessions_provider_session_chk check (
    provider_session_id is null
    or (
      octet_length(provider_session_id) between 8 and 255
      and provider_session_id ~ '^cs_[A-Za-z0-9_]+$'
    )
  ),
  constraint checkout_sessions_url_chk check (
    checkout_url is null or checkout_url like 'https://%'
  ),
  constraint checkout_sessions_status_chk check (
    session_status in ('creating', 'open', 'complete', 'expired')
  ),
  constraint checkout_sessions_payment_status_chk check (
    payment_status in ('unpaid', 'paid', 'no_payment_required')
  ),
  unique (tenant_id, idempotency_key_hash)
);

create unique index if not exists checkout_sessions_provider_session_uq
  on checkout_sessions (tenant_id, provider_session_id)
  where provider_session_id is not null;
create index if not exists checkout_sessions_shard_idx
  on checkout_sessions (shard_key);
create index if not exists checkout_sessions_tenant_created_idx
  on checkout_sessions (tenant_id, created_at desc);
create index if not exists checkout_sessions_reference_idx
  on checkout_sessions (tenant_id, client_reference_id, created_at desc);
create index if not exists checkout_sessions_payment_status_idx
  on checkout_sessions (tenant_id, payment_status, updated_at desc);
