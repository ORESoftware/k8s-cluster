# Runtime realm contract

The Shared Auth server is one codebase deployed as two independent authorities:

```text
shared-auth-admin    -> admin Supabase project    -> admin-auth RDS
shared-auth-customer -> customer Supabase project -> customer-auth RDS
```

`src/realm.rs` validates the selected authority before the HTTP listener opens. The secret-neutral production examples live in `config/auth-realms.contract.json`, and `scripts/validate_auth_realms.py` verifies the contract, declarative schema, and required source guards together.

## Required DB-backed settings

Every database-backed deployment sets all of the following:

| Variable | Admin example | Customer example |
|---|---|---|
| `AUTH_REALM` | `admin` | `customer` |
| `AUTH_REALM_DEPLOYMENT` | `shared-auth-admin` | `shared-auth-customer` |
| `AUTH_ISSUER` | `https://admin-auth.oresoftware.dev` | `https://auth.oresoftware.dev` |
| `AUTH_DATABASE_RESOURCE_REF` | `aws:rds:shared-auth-admin-prod` | `aws:rds:shared-auth-customer-prod` |
| `AUTH_DATABASE_SECRET_REF` | `dd/shared-auth/admin/database-url` | `dd/shared-auth/customer/database-url` |
| `AUTH_SIGNING_KEY_REF` | `dd/shared-auth/admin/signing-key` | `dd/shared-auth/customer/signing-key` |
| `AUTH_SESSION_COOKIE_NAME` | `__Host-shared-auth-admin` | `__Host-shared-auth-customer` |
| `AUTH_REALM_SUPABASE_PROJECT_REF` | dedicated admin project ref | dedicated customer project ref |
| `AUTH_SUPABASE_PROJECTS` | exactly the admin project | exactly the customer project |
| `AUTH_DATABASE_URL` | admin-auth PostgreSQL endpoint | customer-auth PostgreSQL endpoint |

The reference variables are non-secret identity assertions. The actual DSN, signing key, provider credentials, and service credentials remain environment/secret-manager only.

Startup rejects:

- an application-database fallback variable;
- a deployment, database resource, secret path, signing-key path, or cookie name that does not name the selected realm;
- an admin issuer without an `admin-auth` host;
- a customer issuer using the admin host;
- a PostgreSQL host that does not name the selected realm;
- zero, multiple, or mismatched Supabase projects in a production profile.

`AUTH_ALLOW_DBLESS=true` remains an explicit test-only mode. `AUTH_REALM_ALLOW_LOOPBACK=true` permits HTTP plus loopback PostgreSQL only when **both** issuer and database are loopback; setting the flag against normal hosts does not relax the one-project or realm checks. The production JSON contract forbids loopback.

## Customer federation model

The customer RDS holds one global principal and explicit application enrollment:

- `applications` registers a product boundary and enrollment policy;
- `application_accounts` records a principal's status in one application;
- `oauth_clients` registers the exact client, target audience, redirects, allowed scopes, client type, and PKCE requirement;
- `application_consents` records bounded per-application consent;
- `session_application_grants` records which central customer session authorized which exact application/client.

The existing `/auth/delegate` flow and `TokenMinter::mint_delegated` issue short-lived tokens with a target-specific `aud`, `azp`, bounded `scope`, parent-token lineage, preserved session, and preserved assurance. App A and App B therefore receive separate tokens even when the customer realm reuses the same central login ceremony.

Product databases remain authoritative for organizations, workspaces, application roles, billing grants, subscriptions, and resource permissions. Applications validate the customer-realm signature and exact audience locally, then resolve product-local authorization. There are no cross-database joins from product services into Shared Auth.

## Admin boundary

The admin realm has a separate principal/session namespace because it uses a separate RDS instance, issuer, signing key, cookie namespace, Supabase project, and runtime secret set. Customer tokens cannot validate against the admin issuer/key set and customer login cannot create an admin application account.

Administrative policy still needs its own deployment configuration for mandatory strong MFA, shorter sessions, workforce-provider allowlists, step-up, recovery, break-glass, and privileged audit. Those controls must not be represented by an `is_admin` flag on a customer principal.

## Safe rollout

1. Provision both RDS planes and realm-specific secrets without changing traffic.
2. Apply `db/schema.sql` through the declarative database pipeline with a migration role; the server never runs DDL.
3. Create separate admin/customer deployment overlays with the complete settings above.
4. Run startup cross-wire tests, schema tests, cross-realm token rejection, App-A/App-B audience rejection, restore drills, and load tests.
5. Migrate customer applications incrementally, then migrate operators separately with reauthentication/MFA enrollment.
6. Remove legacy auth queries and credentials from the application database only after observation and rollback windows.

The root `deploy/k8s` manifest is the legacy single-realm deployment and is intentionally not rewritten by this implementation PR. Production cutover must use reviewed realm-specific overlays after the new databases and secrets exist; changing the watched manifest early would create an avoidable authentication outage.

## Validation

```sh
python3 -m unittest scripts/test_validate_auth_realms.py
python3 scripts/validate_auth_realms.py \
  --contract config/auth-realms.contract.json \
  --schema db/schema.sql \
  --realm-source src/realm.rs

cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

CI also applies `db/schema.sql` to disposable PostgreSQL before running the Rust suite when a runner is available.
