-- Supabase/PostgREST policy layer for the communications schema.
--
-- Apply only after schema.sql. The portable tables remain the source of truth;
-- this file adds JWT-claim helpers, row-level security, sanitized owner views,
-- and optional grants when Supabase's `authenticated` role exists.
--
-- `raw_user_meta_data` is intentionally not consulted. Ownership is based only
-- on the verified JWT subject and the shared-auth `shared_user_id` claim.

create or replace function communications.jwt_claim(claim_name text)
returns text
language sql
stable
as $$
  select coalesce(
    nullif(current_setting('request.jwt.claim.' || claim_name, true), ''),
    nullif(current_setting('request.jwt.claims', true), '')::jsonb ->> claim_name
  )
$$;

create or replace function communications.owns_identity(
  row_shared_user_id text,
  row_supabase_user_id uuid
)
returns boolean
language sql
stable
as $$
  select
    (row_shared_user_id is not null
      and row_shared_user_id = communications.jwt_claim('shared_user_id'))
    or
    (row_supabase_user_id is not null
      and row_supabase_user_id::text = communications.jwt_claim('sub'))
$$;

alter table communications.endpoints enable row level security;
alter table communications.preferences enable row level security;
alter table communications.jobs enable row level security;
alter table communications.attempts enable row level security;
alter table communications.webhook_requests enable row level security;
alter table communications.receipts enable row level security;
alter table communications.suppressions enable row level security;
alter table communications.outbox enable row level security;

-- Clients register endpoints through the authenticated service API so plaintext
-- targets can be validated and encrypted before persistence. This SELECT policy
-- exists only for the sanitized endpoint_summary view; the table itself is never
-- granted to ordinary clients.
drop policy if exists endpoints_owner_select on communications.endpoints;
create policy endpoints_owner_select
  on communications.endpoints
  for select
  using (communications.owns_identity(shared_user_id, supabase_user_id));

-- Preferences are the only directly mutable client-facing table. The service
-- still validates purpose names, SMS/postal opt-in evidence, and channel order.
drop policy if exists preferences_owner_select on communications.preferences;
create policy preferences_owner_select
  on communications.preferences
  for select
  using (communications.owns_identity(shared_user_id, supabase_user_id));

drop policy if exists preferences_owner_insert on communications.preferences;
create policy preferences_owner_insert
  on communications.preferences
  for insert
  with check (communications.owns_identity(shared_user_id, supabase_user_id));

drop policy if exists preferences_owner_update on communications.preferences;
create policy preferences_owner_update
  on communications.preferences
  for update
  using (communications.owns_identity(shared_user_id, supabase_user_id))
  with check (communications.owns_identity(shared_user_id, supabase_user_id));

drop policy if exists preferences_owner_delete on communications.preferences;
create policy preferences_owner_delete
  on communications.preferences
  for delete
  using (communications.owns_identity(shared_user_id, supabase_user_id));

-- Owners may inspect redacted delivery history. Ciphertext, webhook audit rows,
-- the outbox, and provider correlation internals remain service-only.
drop policy if exists jobs_owner_select on communications.jobs;
create policy jobs_owner_select
  on communications.jobs
  for select
  using (communications.owns_identity(shared_user_id, supabase_user_id));

drop policy if exists attempts_owner_select on communications.attempts;
create policy attempts_owner_select
  on communications.attempts
  for select
  using (
    exists (
      select 1
      from communications.jobs j
      where j.id = attempts.job_id
        and communications.owns_identity(j.shared_user_id, j.supabase_user_id)
    )
  );

drop policy if exists receipts_owner_select on communications.receipts;
create policy receipts_owner_select
  on communications.receipts
  for select
  using (
    exists (
      select 1
      from communications.attempts a
      join communications.jobs j on j.id = a.job_id
      where a.id = receipts.attempt_id
        and communications.owns_identity(j.shared_user_id, j.supabase_user_id)
    )
  );

drop policy if exists suppressions_owner_select on communications.suppressions;
create policy suppressions_owner_select
  on communications.suppressions
  for select
  using (
    shared_user_id = communications.jwt_claim('shared_user_id')
    or exists (
      select 1
      from communications.endpoints e
      where e.id = suppressions.endpoint_id
        and communications.owns_identity(e.shared_user_id, e.supabase_user_id)
    )
  );

create or replace view communications.endpoint_summaries
with (security_invoker = true)
as
select
  id,
  tenant_id,
  application_id,
  shared_user_id,
  supabase_user_id,
  installation_id,
  channel,
  provider,
  provider_environment,
  target_fingerprint,
  target_metadata,
  consent_state,
  verified_at,
  last_seen_at,
  last_success_at,
  last_failure_at,
  last_provider_code,
  status,
  created_at,
  updated_at
from communications.endpoints;

create or replace view communications.user_communication_history
with (security_invoker = true)
as
select
  j.id as job_id,
  j.tenant_id,
  j.application_id,
  j.shared_user_id,
  j.supabase_user_id,
  j.purpose,
  j.contract_version,
  j.template_id,
  j.locale,
  j.content_fingerprint,
  j.state as job_state,
  j.scheduled_at,
  j.accepted_at,
  j.delivered_at,
  j.failed_at,
  j.cancelled_at,
  j.created_at,
  a.id as attempt_id,
  a.attempt_number,
  a.channel,
  a.provider,
  a.provider_environment,
  a.state as attempt_state,
  a.outcome_class,
  a.provider_code,
  a.retry_after_at,
  a.safe_detail,
  a.latency_ms,
  a.started_at,
  a.accepted_at as attempt_accepted_at,
  a.completed_at as attempt_completed_at
from communications.jobs j
left join communications.attempts a on a.job_id = j.id;

revoke all on schema communications from public;
revoke all on all tables in schema communications from public;
revoke all on all functions in schema communications from public;

-- These grants execute only in Supabase-style databases. Service workers use a
-- dedicated database role or the Supabase service role and do not depend on the
-- client grants below.
do $$
begin
  if exists (select 1 from pg_roles where rolname = 'authenticated') then
    grant usage on schema communications to authenticated;
    grant execute on function communications.jwt_claim(text) to authenticated;
    grant execute on function communications.owns_identity(text, uuid) to authenticated;
    grant select, insert, update, delete on communications.preferences to authenticated;
    grant select on communications.endpoint_summaries to authenticated;
    grant select on communications.user_communication_history to authenticated;
  end if;
end
$$;
