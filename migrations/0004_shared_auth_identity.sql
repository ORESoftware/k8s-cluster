-- Central shared-auth identity.
--
-- Human credentials are verified by github.com/shared-auth. This service keeps
-- only the stable shared-auth subject needed to associate a verified identity
-- with its zero-knowledge vault and service-local device sync tokens.

ALTER TABLE threefa.accounts
    ADD COLUMN IF NOT EXISTS shared_auth_user_id UUID;

CREATE UNIQUE INDEX IF NOT EXISTS accounts_shared_auth_user_idx
    ON threefa.accounts (shared_auth_user_id)
    WHERE shared_auth_user_id IS NOT NULL;

-- Replace the earlier Supabase-or-legacy constraint. Shared-auth can represent
-- local and future providers that do not have a Supabase compatibility subject.
ALTER TABLE threefa.accounts
    DROP CONSTRAINT IF EXISTS accounts_identity_present;

ALTER TABLE threefa.accounts
    ADD CONSTRAINT accounts_identity_present CHECK (
        shared_auth_user_id IS NOT NULL
        OR supabase_user_id IS NOT NULL
        OR (username IS NOT NULL AND auth_secret IS NOT NULL)
    );
