BEGIN;

CREATE TABLE IF NOT EXISTS shared_auth.browser_authorization_codes (
    code_hash text PRIMARY KEY,
    client_id text NOT NULL,
    redirect_uri text NOT NULL,
    return_path text NOT NULL,
    supabase_project text NOT NULL,
    code_challenge text NOT NULL,
    encrypted_tokens text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    CONSTRAINT browser_authorization_codes_client_id_length
        CHECK (length(client_id) BETWEEN 1 AND 128),
    CONSTRAINT browser_authorization_codes_pkce_length
        CHECK (length(code_challenge) = 43),
    CONSTRAINT browser_authorization_codes_expiry_order
        CHECK (expires_at > created_at)
);

CREATE INDEX IF NOT EXISTS browser_authorization_codes_expiry_idx
    ON shared_auth.browser_authorization_codes (expires_at)
    WHERE consumed_at IS NULL;

REVOKE ALL ON shared_auth.browser_authorization_codes FROM PUBLIC;

COMMIT;
