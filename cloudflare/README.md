# cloudflare/ — edge session gate

A Cloudflare Worker that validates a caller's session **at the edge**, in tandem
with both auth systems, and redirects to login when there is none.

## Flow

1. **OreSoftware session** (cookie `ore_session` or bearer) → verified locally
   against shared-auth's public JWKS (edge-cached). Fast; works even if Supabase
   is down.
2. **Supabase token fallback** (`x-supabase-token` / `sb-access-token` cookie) →
   exchanged at `shared-auth /auth/exchange` for an OreSoftware token, which is
   set as the `ore_session` cookie. Keeps users signed in even if shared-auth was
   briefly unavailable at login time.
3. **No session** → `302` to `LOGIN_PATH` (browsers) or `401` (API/XHR).

On success the Worker forwards `x-auth-user-id` / `x-auth-project` / `x-auth-email`
to the origin as trusted headers.

## Tandem resilience

- **Supabase down:** step 1 keeps verifying existing OreSoftware sessions; and
  shared-auth itself keeps verifying Supabase tokens from its JWKS cache (see the
  server's grace window). Users with a live session are unaffected.
- **shared-auth down:** the Worker serves the last-known JWKS (stale-within-TTL)
  so existing OreSoftware sessions still verify at the edge; only the *fallback*
  exchange (step 2) is unavailable until it returns.

## Config (`wrangler.toml` vars — no secrets)

`SHARED_AUTH_BASE`, `SHARED_AUTH_JWKS_URL` (optional), `AUTH_ISSUER`,
`AUTH_AUDIENCE`, `LOGIN_PATH`, `JWKS_TTL_SECONDS`.

## Deploy

```bash
cd cloudflare && wrangler deploy
```

Attach routes per app (dashboard or `routes` in `wrangler.toml`).
