# Backend process boundaries

The backend is one Cargo workspace but not one authority boundary. Shared
packages contain mechanisms; separately built binaries own credentials,
network exposure, and operational responsibility.

| Binary | Network ingress | Database identity | Supabase credential | Allowed scope |
| --- | --- | --- | --- | --- |
| `canonical-web-server serve` | HTTP, HTMX, REST, WebSocket | `canonical_web_server` | publishable/legacy anon | Customer authentication and owner-scoped data |
| `canonical-session-revoker run` | none | `canonical_session_revoker` | publishable/legacy anon | Retry and retire durable Supabase logout records |
| `canonical-web-server migrate` | deploy job only | migration owner | none | Apply schema migrations |
| future admin binary | separate origin only | `canonical_admin_server` | isolated admin credential only if required | AAL2 capability functions and immutable audit |

## Compile-time boundary

`services/canonical-session-revoker` depends directly on the auth, config,
session, and store crates. It must not depend on the customer web package,
Axum, Maud, routes, static assets, or WebSocket code. CI inspects its resolved
dependency graph and builds a worker-only container image.

The customer web binary does not run the reconciliation loop. A failed
upstream logout is persisted in `web_session`; the no-ingress worker later
leases that row and retries it. Worker shutdown is cooperative and awaited.

## Database boundary

All scoped database logins are non-owner, `NOBYPASSRLS`, and cannot be reached
through role membership. Bootstrap scripts also reject schema objects owned by
those logins. Each long-lived process rechecks the exact `session_user` and
`current_user`, role attributes, memberships, members, and application-object
ownership at runtime, so privileged `SET ROLE` impersonation fails closed.

- Customer transactions install only a verified user ID and claims JSON with
  transaction-local settings.
- Revocation transactions require the exact `canonical_session_revoker` login,
  clear customer claims, and install the fixed transaction-local
  `canonical.system_task=session_revocation` marker. The `web_session` policy
  uses that marker to catch accidental untagged worker code paths.
- Admin transactions require the exact `canonical_admin_server` login and an
  `aal2` claim. Admin role tables have forced RLS and no customer policies;
  privileged operations must be explicit capability-checking functions.

Never set these markers or claims from unverified request input, and never
simulate a system task by assigning a customer UUID.

The task marker is an audit/accidental-code-path guard, not a secret capability:
a PostgreSQL login can set custom configuration values. Authorization rests on
the exact non-owner, non-`BYPASSRLS`, membership-free revoker login and its
allow-list of `SELECT`, `UPDATE`, and `DELETE` on `web_session` only. Possession
of that isolated database credential is therefore a privileged worker
capability and it must never be mounted into an ingress process.

All role bootstraps clear direct privileges and then reject effective
`PUBLIC`/parent-role table, sequence, and unreviewed function access before
rebuilding an exact process allow-list. The customer runtime receives no
blanket sequence grant; a future object must be reviewed and granted by exact
name/signature to the process that needs it.

## Supabase session boundary

Both active sessions and durable revocation records retain the verified
Supabase `session_id`. Refresh rotation must preserve it. A changed identifier
fails closed, and a locally revoked identifier cannot authenticate through the
bearer-token path while its access JWT remains otherwise valid.

Credential rejection, token-decryption failure, and successful upstream
revocation are distinct terminal outcomes. Only a confirmed success receives
`upstream_revoked_at`; abandoned/dead-letter rows remain auditable and are not
pruned until the access-token horizon and retention window have elapsed.

## Deployment boundary

Build and deploy the two production targets independently:

```sh
docker build --target web -t canonical-web-server .
docker build --target revoker -t canonical-session-revoker .
```

The web image contains the HTTP binary and browser assets. The revoker image
contains only the worker binary, has no exposed port, and uses a distinct
`SESSION_REVOCATION_DATABASE_URL`. Migration credentials are never supplied to
either long-lived process.

Do not deploy an empty admin shell. The first real admin capability should add
a separate binary/origin, its exact database function grants, AAL2 verification,
audit coverage, and capability-specific integration tests in the same change.
