-- Reconcile the reviewed append-only consent contract with projects that
-- previously installed the older broad owner policy and table grants.

drop policy if exists "user_consents_owner" on public.user_consents;

revoke all on table public.user_consents from anon;
revoke all on table public.user_consents from authenticated;
grant select, insert on table public.user_consents to authenticated;
