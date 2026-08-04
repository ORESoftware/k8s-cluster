------------------------------------------------------------------------------
-- Transactionally durable actor attribution for accepted financial operations
--
-- This table records only bounded identity, authorization, assurance, replay,
-- and resource identifiers. Tokens, provider credentials, payment details,
-- request bodies, and raw idempotency keys are forbidden by design.
------------------------------------------------------------------------------

create table if not exists financial_operation_events (
  id                           uuid primary key,
  tenant_id                    uuid not null references tenants(id) on delete restrict,
  operation                    text not null,
  outcome                      text not null,
  actor_kind                   text not null,
  shared_user_id               text,
  shared_session_id            text,
  request_correlation_id       uuid not null,
  authorization_scope          text not null,
  aal                          smallint not null,
  acr                          text,
  auth_time_unix               bigint,
  idempotency_key_fingerprint  text not null,
  resource_type                text not null,
  resource_id                  uuid not null,
  ledger_transaction_id        uuid not null references transactions(id) on delete restrict,
  schema_version               smallint not null default 1,
  occurred_at                  timestamptz not null default now(),

  constraint financial_operation_events_operation check (
    operation = 'ledger.post_transaction'
  ),
  constraint financial_operation_events_outcome check (
    outcome = 'accepted'
  ),
  constraint financial_operation_events_scope check (
    authorization_scope in ('billing:write', 'legacy:service')
  ),
  constraint financial_operation_events_resource check (
    resource_type = 'ledger_transaction'
    and resource_id = ledger_transaction_id
  ),
  constraint financial_operation_events_fingerprint check (
    idempotency_key_fingerprint ~ '^sha256:v1:[0-9a-f]{64}$'
  ),
  constraint financial_operation_events_schema_version check (
    schema_version = 1
  ),
  constraint financial_operation_events_actor check (
    (
      actor_kind = 'shared_auth_user'
      and shared_user_id is not null
      and shared_session_id is not null
      and length(shared_user_id) between 1 and 200
      and length(shared_session_id) between 1 and 200
      and shared_user_id = btrim(shared_user_id)
      and shared_session_id = btrim(shared_session_id)
      and shared_user_id !~ '[[:cntrl:]/\\]'
      and shared_session_id !~ '[[:cntrl:]/\\]'
      and aal in (1, 2)
      and authorization_scope = 'billing:write'
    )
    or (
      actor_kind = 'legacy_service'
      and shared_user_id is null
      and shared_session_id is null
      and aal = 0
      and authorization_scope = 'legacy:service'
    )
  ),
  constraint financial_operation_events_assurance check (
    (
      aal = 2
      and acr = 'urn:oresoftware:loa:2'
      and auth_time_unix > 0
    )
    or (
      aal = 1
      and (acr is null or acr = 'urn:oresoftware:loa:1')
      and auth_time_unix is null
    )
    or (
      aal = 0
      and acr is null
      and auth_time_unix is null
    )
  ),

  unique (tenant_id, operation, ledger_transaction_id),
  unique (tenant_id, operation, idempotency_key_fingerprint)
);

create index if not exists financial_operation_events_tenant_time_idx
  on financial_operation_events (tenant_id, occurred_at desc, id desc);

create index if not exists financial_operation_events_actor_time_idx
  on financial_operation_events (shared_user_id, occurred_at desc, id desc)
  where shared_user_id is not null;

create index if not exists financial_operation_events_correlation_idx
  on financial_operation_events (request_correlation_id);

-- The referenced transaction must belong to the same tenant. The transaction id
-- alone is globally unique, but the explicit tenant check prevents an audit row
-- from claiming another tenant's resource through a malformed direct SQL write.
create or replace function financial_operation_event_check_ledger_tenant()
returns trigger
language plpgsql
as $$
begin
  if not exists (
    select 1
    from transactions
    where id = new.ledger_transaction_id
      and tenant_id = new.tenant_id
  ) then
    raise exception 'financial operation event tenant does not match ledger transaction';
  end if;
  return new;
end;
$$;

drop trigger if exists financial_operation_events_check_ledger_tenant
  on financial_operation_events;
create trigger financial_operation_events_check_ledger_tenant
  before insert on financial_operation_events
  for each row execute function financial_operation_event_check_ledger_tenant();

create or replace function financial_operation_events_immutable()
returns trigger
language plpgsql
as $$
begin
  raise exception 'financial operation events are append-only';
end;
$$;

drop trigger if exists financial_operation_events_no_update
  on financial_operation_events;
create trigger financial_operation_events_no_update
  before update on financial_operation_events
  for each row execute function financial_operation_events_immutable();

drop trigger if exists financial_operation_events_no_delete
  on financial_operation_events;
create trigger financial_operation_events_no_delete
  before delete on financial_operation_events
  for each row execute function financial_operation_events_immutable();
