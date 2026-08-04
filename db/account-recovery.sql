-- Account-recovery schema fragment for the declarative shared_auth contract.
-- Merge this fragment into the canonical pg-defs shared_auth schema before
-- enabling AUTH_RECOVERY_* in a serving deployment. The application runs no DDL.
--
-- Privacy boundary: these tables contain only pseudonymous hashes, opaque
-- provider references, normalized verdicts/confidence values, and audit state.
-- Government-ID images, face frames/templates, voice audio, and speaker
-- embeddings are forbidden from this schema.

create table if not exists shared_auth.biometric_recovery_bindings (
    shared_user_id          uuid        primary key
                                        references shared_auth.principals(shared_user_id)
                                        on delete cascade,
    identity_reference_id   text        not null,
    voice_reference_id      text,
    consent_version         text        not null,
    consented_at            timestamptz not null default now(),
    created_at              timestamptz not null default now(),
    updated_at              timestamptz not null default now(),
    revoked_at              timestamptz,
    check (length(identity_reference_id) between 8 and 512),
    check (voice_reference_id is null or length(voice_reference_id) between 8 and 512),
    check (consent_version ~ '^[A-Za-z0-9._-]{1,64}$')
);

create index if not exists biometric_recovery_bindings_active_idx
    on shared_auth.biometric_recovery_bindings (shared_user_id)
    where revoked_at is null;

create table if not exists shared_auth.account_recovery_ceremonies (
    ceremony_id              uuid        primary key,
    purpose                  text        not null
                                         check (purpose in ('enrollment', 'recovery')),
    shared_user_id           uuid        references shared_auth.principals(shared_user_id)
                                         on delete cascade,
    identifier_hash          text        not null check (length(identifier_hash) = 43),
    ceremony_secret_hash     text        not null unique
                                         check (length(ceremony_secret_hash) = 43),
    identity_session_id      text        not null unique,
    voice_session_id         text        not null unique,
    identity_binding_present boolean     not null default false,
    requires_manual_review   boolean     not null default false,
    consent_version          text        not null,
    status                   text        not null default 'pending'
                                         check (status in (
                                             'pending', 'pending_review', 'cooldown',
                                             'rejected', 'enrolled', 'consumed', 'expired'
                                         )),
    decision_reason          text,
    identity_result_id       text,
    voice_result_id          text,
    identity_reference_id    text,
    voice_reference_id       text,
    document_verified        boolean,
    document_confidence      double precision,
    face_match               boolean,
    face_liveness            boolean,
    face_confidence          double precision,
    advisory_speaker_match   boolean,
    voice_liveness           boolean,
    phrase_match             boolean,
    voice_liveness_confidence double precision,
    advisory_speaker_confidence double precision,
    attempts                 integer     not null default 0 check (attempts between 0 and 10),
    created_at               timestamptz not null default now(),
    updated_at               timestamptz not null default now(),
    expires_at               timestamptz not null,
    available_at             timestamptz,
    consumed_at              timestamptz,
    reviewed_at              timestamptz,
    reviewed_by              text,
    check (length(identity_session_id) between 8 and 128),
    check (length(voice_session_id) between 8 and 128),
    check (consent_version ~ '^[A-Za-z0-9._-]{1,64}$'),
    check (decision_reason is null or length(decision_reason) <= 128),
    check (identity_result_id is null or length(identity_result_id) between 8 and 512),
    check (voice_result_id is null or length(voice_result_id) between 8 and 512),
    check (identity_reference_id is null or length(identity_reference_id) between 8 and 512),
    check (voice_reference_id is null or length(voice_reference_id) between 8 and 512),
    check (document_confidence is null or document_confidence between 0.0 and 1.0),
    check (face_confidence is null or face_confidence between 0.0 and 1.0),
    check (voice_liveness_confidence is null or voice_liveness_confidence between 0.0 and 1.0),
    check (advisory_speaker_confidence is null or advisory_speaker_confidence between 0.0 and 1.0),
    check (reviewed_by is null or length(reviewed_by) between 1 and 128),
    check (expires_at > created_at),
    check (available_at is null or available_at > created_at),
    check (purpose = 'recovery' or shared_user_id is not null),
    check (not identity_binding_present or shared_user_id is not null)
);

create index if not exists account_recovery_identifier_created_idx
    on shared_auth.account_recovery_ceremonies (identifier_hash, purpose, created_at desc);

create index if not exists account_recovery_user_created_idx
    on shared_auth.account_recovery_ceremonies (shared_user_id, created_at desc)
    where shared_user_id is not null;

create index if not exists account_recovery_active_expiry_idx
    on shared_auth.account_recovery_ceremonies (expires_at)
    where status in ('pending', 'pending_review', 'cooldown');
