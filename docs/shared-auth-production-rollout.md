# Shared Auth production rollout

Quaestor Ledger is a financial control plane. A valid Shared Auth identity does
not by itself authorize a billing tenant. Production requests pass two separate
checks:

1. **Shared Auth authentication** — the access token is active, unexpired,
   unrevoked, issued by the configured authority for the configured audience,
   belongs to an active session, and carries a consistent AAL/ACR contract.
2. **Quaestor authorization** — the Shared Auth subject has an active row in
   `tenant_memberships` for the exact tenant and the required billing scope.

Human mutations additionally require a Shared Auth LOA2 token minted no more
than 15 minutes earlier. Refresh-derived LOA1 tokens do not retain step-up.

## Release gates

Do not enable customer billing traffic until all of these are true:

- Shared Auth assurance propagation is deployed and `/auth/introspect` returns
  `iss`, `aud`, `sub`, `sid`, `provider`, `provider_tenant`,
  `provider_subject`, `iat`, `exp`, `aal`, `amr`, and `acr` for active tokens.
- The durable LOA2 ceremony work tracked in **DEN-981** is complete and its
  TOTP/OTP/passkey replay, expiry, restart, and browser suites are green.
- The Quaestor schema diff has been reviewed and rehearsed on a production-like
  shadow database.
- Every pre-existing tenant has at least one explicitly reviewed owner grant.
- The Shared Auth service secret and Quaestor consumer secret are the same
  rotated value, stored only in the approved secret manager.
- PR CI, the authorization model, the clean-runner Rust build, and database
  integration tests are green.

## Required secret fields

AWS Secrets Manager object `dd/remote-dev/billing-server-secrets` must contain:

```text
database_url
master_seal_key_b64
api_auth_bearer
admin_auth_bearer
fiducia_api_key
shared_auth_base_url
shared_auth_introspect_secret
shared_auth_issuer
shared_auth_audience
```

`shared_auth_base_url` must use HTTPS. Plain HTTP is accepted only when an
operator deliberately sets `BILLING_SHARED_AUTH_ALLOW_HTTP=true` for a protected
local or in-cluster development hop; the production overlay does not set it.
The introspection secret must contain at least 32 characters and should be
randomly generated with materially more entropy than that minimum.

The process-wide `api_auth_bearer` is migration/break-glass only. The production
overlay rejects it from tenant routes and hardened mutations. Rotate it during
this rollout and remove it after the remaining non-tenant automation has moved
to scoped service identities.

## Database rollout

Generate and review the declarative migration before deployment:

```bash
export TARGET_DATABASE_URL="$BILLING_DATABASE_URL"
export SHADOW_DATABASE_URL='postgres://.../postgres'

scripts/dpm.sh diff --out /tmp/quaestor-shared-auth.sql
scripts/dpm.sh review
scripts/dpm.sh verify --fail-on-diff
```

Review the generated SQL for these additive objects and invariants:

- `tenant_memberships`
- `tenant_membership_events`
- append-only membership-event triggers
- deferred final-owner protection
- deferred first-owner requirement for newly inserted tenants

Apply only the reviewed artifact through the normal migration job. The
application readiness probe intentionally fails with
`authorization_schema_unavailable` until the membership schema exists.

### Bootstrap existing tenants

The migration does not guess an owner for old tenants. For each existing tenant,
resolve the intended operator's canonical Shared Auth `sub`, review the mapping,
and run:

```bash
psql "$BILLING_DATABASE_URL" \
  --set=tenant_id='11111111-1111-4111-8111-111111111111' \
  --set=shared_user_id='canonical-shared-auth-subject' \
  --file=scripts/bootstrap-tenant-owner.sql
```

Never assign one global owner to every app or GitHub organization merely to
finish the rollout faster. Tenant-by-tenant review limits blast radius and makes
the authorization audit trail meaningful.

Verify there are no ownerless active tenants:

```sql
select t.id, t.slug
from tenants t
left join tenant_memberships m
  on m.tenant_id = t.id
 and m.role = 'owner'
 and m.revoked_at is null
where t.status = 'active'
group by t.id, t.slug
having count(m.shared_user_id) = 0;
```

The query must return zero rows before user-only routing is enabled.

## GitOps deployment

Render the production overlay and inspect it before Argo CD reconciliation:

```bash
kubectl kustomize k8s/ec2 > /tmp/quaestor-rendered.yaml

grep -n 'BILLING_SHARED_AUTH' /tmp/quaestor-rendered.yaml
grep -n 'BILLING_TENANT_ROUTES_REQUIRE_USER_JWT' /tmp/quaestor-rendered.yaml
grep -n 'BILLING_TENANT_MUTATIONS_REQUIRE_STEP_UP' /tmp/quaestor-rendered.yaml
```

Expected production values:

```text
BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true
BILLING_TENANT_MUTATIONS_REQUIRE_STEP_UP=true
BILLING_SHARED_AUTH_REQUIRE_SESSION_ID=true
BILLING_ADMIN_UI_ENABLED=false
```

Direct `BILLING_SUPABASE_URL` verification must not appear in the final
container environment. Shared Auth owns provider JWT verification and session
revocation.

Deploy the schema first, then the application. Do not send customer traffic to a
pod until both `/readyz` and `dd_billing_server_authz_schema_ready` report ready.

## Smoke and negative tests

Use distinct tenants A and B and at least three Shared Auth subjects: owner A,
reader A, and owner B.

1. No `Authorization` header returns `401` on protected routes.
2. An inactive, expired, revoked, wrong-issuer, wrong-audience, or sessionless
   token returns `401`; an unavailable/misconfigured introspection authority
   returns `503`, never an authorization bypass.
3. Owner A can read tenant A and receives `403` for tenant B.
4. Reader A can read tenant A and receives `403` for every mutation.
5. An LOA1 token with `billing:write` receives `403` for a mutation.
6. An LOA2 token older than 15 minutes receives `403` for a mutation.
7. A fresh LOA2 token plus tenant A `billing:write` can perform an idempotent
   test mutation in tenant A and still receives `403` in tenant B.
8. OAuth and Plaid setup reject a tenant identifier not present in the caller's
   exact grant.
9. Membership administration requires `billing:admin`, writes an append-only
   event, rejects self-revocation, and cannot remove the final owner.
10. Provider webhooks remain governed by provider signature/idempotency checks;
    public proof endpoints remain public. Neither surface accepts a user bearer
    as a substitute for its own trust boundary.

Record correlation IDs, expected/actual status codes, and the corresponding
membership audit rows in the release evidence.

## Observability

Alert on:

- sustained Shared Auth introspection `503` responses or latency above the
  configured 1.5-second request timeout;
- spikes in protected-route `401`/`403` grouped by route and tenant, without
  logging access tokens;
- `dd_billing_server_authz_schema_ready != 1`;
- ownerless-tenant invariant violations or failed membership transactions;
- membership grant/revoke volume outside an approved change window;
- financial mutation failures, reconciliation breaks, and unbalanced-posting
  invariant failures.

Access tokens, provider credentials, service secrets, raw OTPs, passkey
challenge material, and sealed payment credentials must never appear in logs,
traces, metrics labels, or audit payloads.

## Rollback

The membership migration is additive; do not drop its tables or triggers during
an application rollback. If the new application cannot remain deployed:

1. stop new customer mutations;
2. roll back the application image while preserving the membership schema and
   audit history;
3. keep the static admin UI disabled;
4. rotate any Shared Auth introspection secret whose exposure is suspected;
5. reconcile all requests accepted during the rollout window before resuming.

Do not disable `BILLING_TENANT_ROUTES_REQUIRE_USER_JWT` or
`BILLING_TENANT_MUTATIONS_REQUIRE_STEP_UP` as a convenience rollback. That
would restore a known cross-tenant/global-secret risk. A safe rollback reduces
traffic or restores the prior image; it does not weaken authorization.

## Billing-platform boundary

This rollout makes the existing ledger/reconciliation service safe to expose to
Shared Auth principals. It does **not** by itself provide the complete customer
billing domain. Product/price revisions, subscriptions, usage metering,
invoices, tax snapshots, credits, collections/dunning, and end-to-end close and
recovery evidence are tracked under **DEN-1427 through DEN-1432** and remain
launch requirements before Quaestor bills real customers across all apps and
GitHub organizations.
