-- shared_auth — declarative schema for the OreSoftware shared auth server.
--
-- Owned by pg-defs and applied with dpm (declarative; no migration files). The
-- server connects with search_path=shared_auth and runs NO DDL. One namespace
-- per app, per the org convention (see the pg-defs + dpm memory).
--
-- Postgres is the authoritative shared-auth store. External identity providers
-- (Supabase today; Clerk/Cognito may be added later) are linked through
-- provider_identities rather than being baked into the principals table. Passwords
-- are never stored: local_credentials contains Argon2id PHC strings only.

create schema if not exists shared_auth;

create table if not exists shared_auth.principals (
    shared_user_id    uuid        primary key default gen_random_uuid(),
    email             text,
    email_verified    boolean     not null default false,
    phone             text,
    phone_verified    boolean     not null default false,
    display_name      text,
    status            text        not null default 'active'
                                  check (status in ('active', 'disabled', 'deleted')),
    profile           jsonb       not null default '{}'::jsonb,
    created_at        timestamptz not null default now(),
    updated_at        timestamptz not null default now(),
    last_seen_at      timestamptz not null default now(),
    check (email is null or (length(email) between 3 and 320)),
    check (phone is null or length(phone) <= 64),
    check (display_name is null or length(display_name) <= 160)
);

create unique index if not exists users_email_unique_idx
    on shared_auth.principals (lower(email))
    where email is not null and status <> 'deleted';

create index if not exists users_status_idx
    on shared_auth.principals (status);

create table if not exists shared_auth.provider_identities (
    provider_identity_id uuid        primary key default gen_random_uuid(),
    shared_user_id       uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    provider             text        not null,
    provider_tenant      text        not null default 'default',
    provider_subject     text        not null,
    email                text,
    email_verified       boolean     not null default false,
    metadata             jsonb       not null default '{}'::jsonb,
    created_at           timestamptz not null default now(),
    updated_at           timestamptz not null default now(),
    last_seen_at         timestamptz not null default now(),
    unique (provider, provider_tenant, provider_subject),
    check (length(provider) between 1 and 64),
    check (length(provider_tenant) between 1 and 255),
    check (length(provider_subject) between 1 and 512)
);

create index if not exists provider_identities_user_idx
    on shared_auth.provider_identities (shared_user_id);

create table if not exists shared_auth.local_credentials (
    shared_user_id       uuid        primary key references shared_auth.principals(shared_user_id) on delete cascade,
    password_hash        text        not null,
    password_changed_at  timestamptz not null default now(),
    failed_attempts      integer     not null default 0 check (failed_attempts >= 0),
    locked_until         timestamptz,
    created_at           timestamptz not null default now(),
    updated_at           timestamptz not null default now(),
    check (length(password_hash) between 40 and 512)
);

create table if not exists shared_auth.sessions (
    session_id          uuid        primary key default gen_random_uuid(),
    shared_user_id      uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    refresh_token_hash  text        not null unique,
    provider            text        not null,
    provider_tenant     text        not null default 'default',
    provider_subject    text        not null,
    auth_level          smallint    not null default 1 check (auth_level in (1, 2)),
    auth_methods        jsonb       not null default '[]'::jsonb,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    last_seen_at        timestamptz not null default now(),
    expires_at          timestamptz not null,
    revoked_at          timestamptz,
    rotated_from        uuid        references shared_auth.sessions(session_id) on delete set null,
    check (length(refresh_token_hash) = 43),
    check (jsonb_typeof(auth_methods) = 'array'),
    check (expires_at > created_at)
);

create index if not exists sessions_user_idx
    on shared_auth.sessions (shared_user_id);
create index if not exists sessions_active_expiry_idx
    on shared_auth.sessions (expires_at)
    where revoked_at is null;

-- Passwordless email tokens are opaque, single-use, short-lived credentials.
-- Only the SHA-256 hash is persisted; the plaintext exists only long enough to
-- be placed in the SendGrid message.
create table if not exists shared_auth.magic_link_tokens (
    token_hash          text        primary key check (length(token_hash) = 43),
    otp_hash            text        not null check (length(otp_hash) = 43),
    shared_user_id      uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    identifier_hash     text        not null check (length(identifier_hash) = 43),
    failed_attempts     integer     not null default 0 check (failed_attempts between 0 and 5),
    created_at          timestamptz not null default now(),
    expires_at          timestamptz not null,
    consumed_at         timestamptz,
    check (expires_at > created_at)
);

create index if not exists magic_link_tokens_user_idx
    on shared_auth.magic_link_tokens (shared_user_id);
create index if not exists magic_link_tokens_active_expiry_idx
    on shared_auth.magic_link_tokens (expires_at)
    where consumed_at is null;
create index if not exists magic_link_tokens_identifier_created_idx
    on shared_auth.magic_link_tokens (identifier_hash, created_at desc);

create table if not exists shared_auth.mfa_sms_challenges (
    challenge_id        uuid        primary key default gen_random_uuid(),
    shared_user_id      uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    phone_e164          text        not null check (phone_e164 ~ '^\+[1-9][0-9]{7,14}$'),
    created_at          timestamptz not null default now(),
    expires_at          timestamptz not null,
    verified_at         timestamptz,
    check (expires_at > created_at)
);

create index if not exists mfa_sms_challenges_user_idx
    on shared_auth.mfa_sms_challenges (shared_user_id, created_at desc);
create index if not exists mfa_sms_challenges_active_expiry_idx
    on shared_auth.mfa_sms_challenges (expires_at)
    where verified_at is null;

create table if not exists shared_auth.roles (
    role_id           uuid        primary key default gen_random_uuid(),
    shared_user_id    uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    role_name         text        not null check (role_name ~ '^[a-z][a-z0-9:_-]{0,63}$'),
    granted_at        timestamptz not null default now(),
    granted_by        uuid        references shared_auth.principals(shared_user_id) on delete set null,
    unique (shared_user_id, role_name)
);

create index if not exists roles_user_idx
    on shared_auth.roles (shared_user_id);

-- The customer realm owns one global principal and a separate enrollment in
-- each first-party application. SSO reuses the central login ceremony, but each
-- application receives its own audience-scoped token and may deny enrollment.
-- Product authorization remains in each application database; these tables do
-- not own organization membership, billing authority, or resource permissions.
create table if not exists shared_auth.applications (
    application_id     uuid        primary key default gen_random_uuid(),
    application_key    text        not null unique
                                  check (application_key ~ '^[a-z][a-z0-9-]{1,63}$'),
    display_name       text        not null check (length(display_name) between 1 and 160),
    status             text        not null default 'active'
                                  check (status in ('active', 'disabled')),
    enrollment_policy  text        not null default 'automatic'
                                  check (enrollment_policy in ('automatic', 'invite', 'disabled')),
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now()
);

create table if not exists shared_auth.application_accounts (
    application_id       uuid        not null references shared_auth.applications(application_id) on delete cascade,
    shared_user_id       uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    status               text        not null default 'active'
                                     check (status in ('active', 'suspended', 'deleted')),
    profile              jsonb       not null default '{}'::jsonb,
    created_at           timestamptz not null default now(),
    updated_at           timestamptz not null default now(),
    last_authenticated_at timestamptz,
    primary key (application_id, shared_user_id),
    check (jsonb_typeof(profile) = 'object')
);

create index if not exists application_accounts_user_idx
    on shared_auth.application_accounts (shared_user_id, status);

create table if not exists shared_auth.oauth_clients (
    client_id           text        primary key
                                  check (client_id ~ '^[A-Za-z0-9][A-Za-z0-9._:-]{2,127}$'),
    application_id      uuid        not null references shared_auth.applications(application_id) on delete cascade,
    audience            text        not null unique
                                  check (audience ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]{2,127}$'),
    client_type         text        not null default 'public'
                                  check (client_type in ('public', 'confidential')),
    client_secret_hash  text,
    redirect_uris       jsonb       not null default '[]'::jsonb,
    allowed_scopes      jsonb       not null default '[]'::jsonb,
    require_pkce        boolean     not null default true,
    status              text        not null default 'active'
                                  check (status in ('active', 'disabled')),
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    unique (application_id, client_id),
    check (jsonb_typeof(redirect_uris) = 'array'),
    check (jsonb_typeof(allowed_scopes) = 'array'),
    check (
        (client_type = 'public' and client_secret_hash is null and require_pkce)
        or
        (client_type = 'confidential' and client_secret_hash is not null
         and length(client_secret_hash) between 43 and 512)
    )
);

create index if not exists oauth_clients_application_idx
    on shared_auth.oauth_clients (application_id, status);

create table if not exists shared_auth.application_consents (
    application_id     uuid        not null references shared_auth.applications(application_id) on delete cascade,
    shared_user_id     uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    scopes             jsonb       not null default '[]'::jsonb,
    granted_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now(),
    revoked_at         timestamptz,
    primary key (application_id, shared_user_id),
    check (jsonb_typeof(scopes) = 'array')
);

create index if not exists application_consents_user_idx
    on shared_auth.application_consents (shared_user_id, revoked_at);

-- A central browser session can authorize several applications without sharing
-- cookies or bearer tokens between them. Each grant names the exact registered
-- client; revoking one grant need not terminate unrelated application sessions.
create table if not exists shared_auth.session_application_grants (
    session_id         uuid        not null references shared_auth.sessions(session_id) on delete cascade,
    application_id     uuid        not null,
    client_id          text        not null,
    granted_at         timestamptz not null default now(),
    last_used_at       timestamptz not null default now(),
    revoked_at         timestamptz,
    primary key (session_id, application_id, client_id),
    foreign key (application_id, client_id)
        references shared_auth.oauth_clients(application_id, client_id) on delete cascade
);

create index if not exists session_application_grants_active_idx
    on shared_auth.session_application_grants (application_id, client_id, last_used_at desc)
    where revoked_at is null;

-- Browser authorization codes are opaque, PKCE-bound and single-use. Supabase
-- token bundles are AES-256-GCM ciphertext; plaintext tokens never enter URLs.
create table if not exists shared_auth.browser_authorization_codes (
    code_hash           text        primary key check (length(code_hash) = 43),
    client_id           text        not null check (length(client_id) between 1 and 128),
    redirect_uri        text        not null check (length(redirect_uri) between 1 and 512),
    return_path         text        not null check (length(return_path) between 1 and 512),
    supabase_project    text        not null check (length(supabase_project) between 1 and 128),
    code_challenge      text        not null check (length(code_challenge) = 43),
    encrypted_tokens    text        not null check (length(encrypted_tokens) between 64 and 65536),
    created_at          timestamptz not null default now(),
    expires_at          timestamptz not null,
    consumed_at         timestamptz,
    check (expires_at > created_at),
    check (consumed_at is null or consumed_at >= created_at)
);

create index if not exists browser_authorization_codes_active_expiry_idx
    on shared_auth.browser_authorization_codes (expires_at)
    where consumed_at is null;

-- Enrolled MFA factors. TOTP seeds are AES-256-GCM ciphertext and nonce; passkeys
-- contain only the serialised public credential returned by webauthn-rs. Raw
-- fingerprint/face material is never accepted or stored.
create table if not exists shared_auth.auth_factors (
    factor_id          uuid        primary key default gen_random_uuid(),
    shared_user_id     uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    kind               text        not null check (kind in ('totp', 'passkey')),
    label              text,
    secret_ciphertext  bytea,
    secret_nonce       bytea,
    public_data        jsonb       not null default '{}'::jsonb,
    external_id        text,
    enabled            boolean     not null default false,
    confirmed_at       timestamptz,
    last_used_at       timestamptz,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now(),
    check (label is null or length(label) <= 160),
    check (external_id is null or length(external_id) <= 2048),
    check (
        (kind = 'totp' and secret_ciphertext is not null and secret_nonce is not null)
        or
        (kind = 'passkey' and secret_ciphertext is null and secret_nonce is null and external_id is not null)
    )
);

create index if not exists auth_factors_user_idx
    on shared_auth.auth_factors (shared_user_id, kind, enabled);
create unique index if not exists auth_factors_external_id_unique_idx
    on shared_auth.auth_factors (kind, external_id)
    where external_id is not null;

-- Short-lived server-side challenge state. OTP codes are represented only by a
-- keyed tag; WebAuthn registration/authentication state is JSON owned by the
-- server and consumed exactly once.
create table if not exists shared_auth.auth_challenges (
    challenge_id       uuid        primary key default gen_random_uuid(),
    shared_user_id     uuid        not null references shared_auth.principals(shared_user_id) on delete cascade,
    session_id         uuid        not null references shared_auth.sessions(session_id) on delete cascade,
    kind               text        not null check (kind in ('email_otp', 'sms_otp', 'passkey_register', 'passkey_auth')),
    destination_hint   text,
    code_tag           bytea,
    state              jsonb       not null default '{}'::jsonb,
    attempts           integer     not null default 0 check (attempts >= 0),
    max_attempts       integer     not null check (max_attempts between 1 and 20),
    expires_at         timestamptz not null,
    consumed_at        timestamptz,
    created_at         timestamptz not null default now(),
    check (expires_at > created_at),
    check (
        (kind in ('email_otp', 'sms_otp') and code_tag is not null)
        or
        (kind in ('passkey_register', 'passkey_auth') and code_tag is null)
    )
);

create index if not exists auth_challenges_active_idx
    on shared_auth.auth_challenges (shared_user_id, session_id, kind, expires_at)
    where consumed_at is null;

-- HMAC-authenticated sync events are recorded before they are applied. The
-- primary key makes webhook retries idempotent across all replicas.
create table if not exists shared_auth.webhook_events (
    event_id           uuid        primary key,
    provider           text        not null,
    event_type         text        not null,
    received_at        timestamptz not null default now(),
    payload_sha256     text        not null check (length(payload_sha256) = 43)
);

create index if not exists webhook_events_received_idx
    on shared_auth.webhook_events (received_at);
