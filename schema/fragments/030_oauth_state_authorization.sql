------------------------------------------------------------------------------
-- Bind each OAuth callback capability to the Shared Auth principal and the
-- actual MFA/passkey ceremony that authorized it.
--
-- Existing rows are intentionally left nullable during a rolling migration and
-- are rejected by the callback handler. oauth_states are short lived (15m), so
-- operators may also drain/delete them before enabling the new callback code.
------------------------------------------------------------------------------

alter table oauth_states
  add column if not exists initiating_shared_user_id text,
  add column if not exists auth_time_unix bigint;

alter table oauth_states
  drop constraint if exists oauth_states_authorization_binding;

alter table oauth_states
  add constraint oauth_states_authorization_binding check (
    (
      initiating_shared_user_id is null
      and auth_time_unix is null
    )
    or (
      length(initiating_shared_user_id) between 1 and 200
      and auth_time_unix > 0
    )
  );

create index if not exists oauth_states_authorization_idx
  on oauth_states (tenant_id, initiating_shared_user_id, expires_at)
  where initiating_shared_user_id is not null;