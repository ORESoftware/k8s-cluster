# Shared Auth production contract

Quaestor Ledger verifies Shared Auth access tokens locally and performs product authorization inside the billing service. Direct Supabase verification remains available only as an explicit migration mode.

## Required production configuration

```text
BILLING_SHARED_AUTH_URL=http://dd-shared-auth.shared-auth.svc.cluster.local:8120
BILLING_SHARED_AUTH_ISSUER=<exact AUTH_ISSUER configured on shared-auth>
BILLING_SHARED_AUTH_AUDIENCE=<exact AUTH_AUDIENCE configured on shared-auth>
# Optional when BILLING_SHARED_AUTH_URL is set:
# BILLING_SHARED_AUTH_JWKS_URL=http://dd-shared-auth.shared-auth.svc.cluster.local:8120/.well-known/jwks.json

BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true
BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS=true
```

The issuer and audience are exact string constraints. Do not derive either from the request host or accept a list of unrelated issuers. Shared Auth must use asymmetric signing; the Shared Auth path deliberately ignores `BILLING_SUPABASE_JWT_SECRET` and rejects HS256.

`BILLING_API_AUTH_BEARER` remains required by the current startup contract for internal provisioning and migration callers. It is not sufficient for `/v1/tenants/{tenant_id}/...` when `BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true`, and it is rejected for protected tenant financial mutations. Never embed it in browser, mobile, or desktop clients.

## Shared Auth roles

Shared Auth Postgres is the source of truth for role grants. Quaestor recognizes only these namespaced values:

- `quaestor:tenant:<tenant-uuid>` — membership in exactly one billing tenant;
- `quaestor:billing:read` — reserved read permission vocabulary;
- `quaestor:billing:write` — mutation permission; fresh AAL2 is still required;
- `quaestor:reconciliation:run` — reserved reconciliation permission;
- `quaestor:billing:admin` — reserved administrative permission; never bypasses tenant membership or step-up.

The current middleware requires tenant membership on all tenant-scoped routes and maps only `quaestor:billing:write` to the existing `billing:write` mutation scope. `provider_tenant`, provider project names, email domains, and global `user`/`admin` roles are not tenant entitlements.

Example grants for a billing operator:

```sql
insert into shared_auth.roles (shared_user_id, role_name)
values
  (:shared_user_id, 'quaestor:tenant:11111111-1111-1111-1111-111111111111'),
  (:shared_user_id, 'quaestor:billing:write')
on conflict do nothing;
```

Grant changes should be made through reviewed operator tooling or an HMAC-authenticated synchronization path, not directly from product clients.

## Assurance and freshness

A financial mutation requires all of the following:

1. a valid Shared Auth signature from the pinned JWKS;
2. the exact configured issuer and audience;
3. `quaestor:tenant:<requested-tenant>`;
4. `quaestor:billing:write`;
5. AAL2 (`aal=2` and `acr=urn:oresoftware:loa:2`);
6. `auth_time` no more than 15 minutes old.

Future `auth_time` values beyond the 30-second clock-skew allowance fail closed. A refresh token returns the Shared Auth session to AAL1, so a prior MFA ceremony cannot be extended indefinitely through refresh.

## Key rotation and authority outages

The verifier caches JWKS for 10 minutes, rate-limits refresh attempts to one every 30 seconds, refuses redirects, limits the response to 256 KiB, and checks `kid`, JWK `alg`, and JWK use together. During a transient authority outage it may use the same matching key for at most 20 minutes from the last successful fetch. An unknown `kid` never falls back to another key.

This keeps ordinary requests available during a brief Shared Auth outage while bounding emergency key-revocation delay. For immediate session revocation and account-disable propagation, add authenticated `/auth/introspect` checks after the Shared Auth fail-closed introspection change is merged; fail closed for financial mutations when that revocation authority is unavailable.

## Direct Supabase migration

The old `BILLING_SUPABASE_*` verifier is disabled in production by default. A time-boxed migration deployment may set:

```text
BILLING_ALLOW_DIRECT_SUPABASE_AUTH=true
```

Legacy tokens use `app_metadata.tenant_id` / `tenant_ids`, `app_metadata.financial_scopes`, string `aal`, and timestamped AMR entries. Remove the flag after clients exchange provider tokens through Shared Auth. Do not enable both authorities as a permanent steady state.
