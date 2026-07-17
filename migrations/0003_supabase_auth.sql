-- Supabase-backed identity.
--
-- Until now an account was username + Argon2id verifier ("legacy" auth). We add
-- Supabase as a second, preferred identity source: Supabase Auth owns the login
-- (email/password, OAuth, MFA), and this server verifies the Supabase-issued JWT
-- and maps its `sub` (the Supabase user id) onto a local account. The vault stays
-- zero-knowledge: identity moving to Supabase does not give the server any vault
-- key material.
--
-- An account is now valid if it has EITHER legacy credentials OR a Supabase user
-- id. Legacy columns become nullable so a Supabase-only account needs no local
-- password (which was the E2E-key/login-credential conflation we are removing).

ALTER TABLE threefa.accounts
    ADD COLUMN IF NOT EXISTS supabase_user_id UUID,
    ADD COLUMN IF NOT EXISTS email            TEXT;

-- A given Supabase user maps to exactly one local account.
CREATE UNIQUE INDEX IF NOT EXISTS accounts_supabase_user_idx
    ON threefa.accounts (supabase_user_id)
    WHERE supabase_user_id IS NOT NULL;

-- Legacy credentials are optional for Supabase-only accounts.
ALTER TABLE threefa.accounts ALTER COLUMN username    DROP NOT NULL;
ALTER TABLE threefa.accounts ALTER COLUMN auth_secret DROP NOT NULL;

-- Every account must be reachable by *some* identity: legacy creds or Supabase.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.constraint_column_usage
        WHERE table_schema = 'threefa' AND table_name = 'accounts'
          AND constraint_name = 'accounts_identity_present'
    ) THEN
        ALTER TABLE threefa.accounts
            ADD CONSTRAINT accounts_identity_present CHECK (
                supabase_user_id IS NOT NULL
                OR (username IS NOT NULL AND auth_secret IS NOT NULL)
            );
    END IF;
END
$$;

-- Track device recency so `GET /v1/devices` can show last-seen and a user can
-- recognize and revoke stale enrollments.
ALTER TABLE threefa.devices
    ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ;
