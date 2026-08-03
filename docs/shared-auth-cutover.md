# Shared Auth production cutover

Quaestor Ledger accepts customer-facing JWTs only through the existing pinned
issuer/JWKS verifier in `src/supabase_auth.rs`. In production, that verifier must
be mapped to the centralized Shared Auth issuer rather than directly to one
application's Supabase project.

The runtime check in `src/shared_auth.rs` enforces that mapping before the
process opens the ledger database, sealing key, provider credentials, NATS, or
scheduler.

## Required issuer contract

Deploy the Shared Auth release that emits token contract version 1 before
cutting Quaestor over. A current access token contains:

- ES256 signature with a `kid` published at `/.well-known/jwks.json`
- pinned `iss` and `aud`
- `sub`, `iat`, `nbf`, `exp`, UUID `jti`, `token_use=access`, and `ver=1`
- normalized `aal` and bounded `amr` timestamps
- canonical tenant UUIDs in `app_metadata.tenant_ids`
- financial grants in `app_metadata.financial_scopes`

Shared Auth derives tenant IDs and financial scopes only from the verified
upstream token's client-unwritable `app_metadata`. User-writable
`user_metadata` is never an authorization source.

## Production environment

The first group describes the intended Shared Auth authority. The second group
maps the billing server's existing generic verifier settings to exactly the same
values. The duplication is intentional during this compatibility phase: startup
fails if the two groups drift.

```bash
BILLING_SHARED_AUTH_REQUIRED=true
BILLING_SHARED_AUTH_BASE_URL=https://auth.oresoftware.dev
BILLING_SHARED_AUTH_ISSUER=https://auth.oresoftware.dev
BILLING_SHARED_AUTH_JWKS_URL=https://auth.oresoftware.dev/.well-known/jwks.json
BILLING_SHARED_AUTH_AUDIENCE=oresoftware

BILLING_SUPABASE_URL=https://auth.oresoftware.dev
BILLING_SUPABASE_JWT_ISS=https://auth.oresoftware.dev
BILLING_SUPABASE_JWKS_URL=https://auth.oresoftware.dev/.well-known/jwks.json
BILLING_SUPABASE_JWT_AUD=oresoftware

BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true
BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS=true
```

`BILLING_SUPABASE_JWT_SECRET` must be absent. Shared Auth access tokens are
asymmetric ES256 tokens; configuring an HS256 secret widens the accepted
algorithm set and causes startup to fail.

`BILLING_API_AUTH_BEARER` remains a separate high-entropy service credential for
unscoped service-to-service provisioning calls. It is not an end-user token and
must never be embedded in web, desktop, or mobile clients. Existing
authorization logic denies that shared credential tenant mutations once the
financial step-up gate is enabled.

Local development may set `BILLING_ALLOW_INSECURE_DEV=1`; that makes Shared Auth
optional by default. Set `BILLING_SHARED_AUTH_REQUIRED=true` alongside it to
exercise the production contract against a local HTTPS-capable fixture.

## Ordered rollout

1. Deploy Shared Auth token contract v1 and leave current consumers unchanged.
2. Fetch its JWKS and verify the advertised `kid`, `kty=EC`, `alg=ES256`, and
   `use=sig`.
3. Exchange a test user's upstream Supabase token. Introspect the resulting
   Shared Auth token and verify the expected tenant ID, `billing:write`, `aal2`,
   and a recent AMR timestamp.
4. Add the environment mapping above to a non-production Quaestor deployment.
5. Exercise the authorization matrix below.
6. Deploy the same immutable images and configuration to production.
7. After at least one maximum access-token TTL, remove any gateway path that
   still forwards direct Supabase tokens to Quaestor.

Do not reverse steps 1 and 4. Tokens minted by the previous Shared Auth contract
carry stable identity but no tenant/scoped authorization, so Quaestor correctly
rejects them rather than treating absent claims as wildcards.

## Required authorization matrix

Use two real tenants and at least three test sessions:

| Request | Expected |
| --- | --- |
| entitled tenant read with valid Shared Auth token | 2xx |
| different tenant ID with the same token | 403 |
| entitled tenant mutation with AAL2, fresh AMR, `billing:write` | application result |
| entitled tenant mutation with AAL1 | 403 |
| entitled tenant mutation with stale or absent AMR timestamp | 403 |
| entitled tenant mutation without `billing:write` | 403 |
| tenant mutation using only `BILLING_API_AUTH_BEARER` | 403 |
| missing, malformed, expired, wrong-issuer, or wrong-audience token | 401 |
| valid token while JWKS has never been fetched and issuer is unavailable | 503/401 fail-closed |
| signed provider webhook on its exempt callback path | unchanged provider-specific result |

Also verify that a token containing tenant/scopes only in `user_metadata` reaches
no tenant. The standalone formal-contract test in
`formal/ledger-model/tests/shared_auth_contract.rs` permanently gates this
trust boundary in CI.

## Rollback

Prefer rolling back the last billing deployment/configuration as one unit. Never
restore tenant access by setting `BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false`
or by allowing the global service bearer to mutate tenant state.

For an issuer-specific incident, the bounded emergency path is:

1. block customer mutation routes at the gateway;
2. set `BILLING_SHARED_AUTH_REQUIRED=false` only while restoring the previous
   direct asymmetric issuer mapping;
3. keep tenant JWT enforcement and fresh-AAL2/`billing:write` enforcement on;
4. restore Shared Auth, repeat the authorization matrix, then set
   `BILLING_SHARED_AUTH_REQUIRED=true` again;
5. record the duration and affected token IDs in the incident timeline.

This bypass disables only the explicit issuer-mapping assertion. It must not be
combined with the tenant-JWT or financial-step-up migration bypasses.

## Operational checks

- Alert on startup failure mentioning `BILLING_SHARED_AUTH_*`; it indicates
  issuer/config drift, not a transient database problem.
- Alert on sustained 401/403 changes by route and tenant, without logging bearer
  tokens or raw claims.
- Track Shared Auth exchange failures and JWKS refresh failures separately.
- Rotate signing keys with an overlap window: publish the new public key before
  minting with its `kid`, retain the old key until every old token has expired,
  then remove it.
- Keep Shared Auth and Quaestor clocks synchronized. Both services allow only a
  small skew; AMR freshness is deliberately measured in seconds, not as a broad
  grace period.
