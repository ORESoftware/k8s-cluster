# Browser authorization-code handoff

Product applications use shared-auth as a multi-tenant sign-in broker without
putting Supabase access or refresh tokens in browser URLs.

## Protocol

1. The product backend creates a high-entropy `state` and PKCE verifier, stores
   both in short-lived `HttpOnly; Secure; SameSite=Lax` cookies, and redirects to
   `GET /authorize` with the S256 challenge.
2. shared-auth validates the exact registered `client_id`, `redirect_uri`, and
   local `return_to` path. It signs the user into the Supabase project assigned
   to that client and verifies the returned access token against that project's
   issuer/JWKS.
3. shared-auth encrypts the Supabase access and refresh tokens with
   `AUTH_HANDOFF_ENCRYPTION_KEY`, stores them behind a hashed 90-second opaque
   code, and redirects only the code plus `state` to the registered callback.
4. The product backend validates `state` and redeems the code at
   `POST /auth/handoff/redeem` using its client secret and PKCE verifier. The SQL
   update marks the code consumed before returning the decrypted token bundle.
5. The product creates its normal origin-scoped encrypted session cookie and
   clears the temporary PKCE/state cookies.

## Configuration

`AUTH_BROWSER_CLIENTS` contains metadata only; secret values are referenced by
environment-variable name:

```json
[
  {
    "client_id": "canonical-plus",
    "supabase_project": "canonical-plus",
    "redirect_uris": [
      "https://app.canonical.plus/auth/shared/callback"
    ],
    "return_paths": ["/u/quote"],
    "client_secret_env": "AUTH_BROWSER_CANONICAL_PLUS_SECRET"
  }
]
```

Required secret values:

- `AUTH_HANDOFF_ENCRYPTION_KEY`: base64 encoding of 32 random bytes.
- Each `client_secret_env`: at least 32 random bytes, shared only with that
  product backend.

Optional `AUTH_HANDOFF_CODE_TTL_SECS` is constrained to 30–300 seconds and
defaults to 90.

The schema is declarative. Apply the updated `db/schema.sql` through the normal
pg-defs/dpm deployment path before enabling a browser client; the server itself
runs no DDL. Expired and consumed rows may be deleted by a low-priority
maintenance job.
