# fiducia-customer

The canonical customer web application and BFF for
[fiducia.cloud](https://fiducia.cloud). It is a Rust MASH deployment:

> [!NOTE]
> **Naming.** This repository was renamed from `fiducia-backend.rs` to
> `fiducia-customer.rs` (2026-07) — it is the *customer* app, not a shared
> backend. The deployed runtime identity intentionally still uses the historical
> name: the crate/binary is `fiducia-backend`, the test image is
> `ghcr.io/fiducia-cloud/fiducia-backend`, and the k8s Deployment/Service/HPA in
> the `fiducia` namespace are `fiducia-backend`. Renaming those is a live-infra
> change tracked separately; GitHub redirects the old repo URL.

- **Maud** renders escaped, compile-checked customer HTML.
- **Axum** owns routes, middleware, cookies, WebSocket/SSE refresh endpoints,
  and server-mediated login.
- **SeaORM** owns customer profile, preference, and local session persistence in
  Postgres.
- **HTMX** progressively enhances same-origin forms and authenticated fragments.

The sibling `fiducia-marketing.web` is the static Astro marketing site only; its build
is the fallback for the public host. The deprecated `fiducia-customer-ui.web`
SPA is preserved for history but is not loaded or deployed by this service.

## Customer and admin are separate

| Boundary | Customer | Admin |
|---|---|---|
| Repository | `fiducia-customer.rs` | `fiducia-admin.rs` |
| Cookie | release `__Host-fiducia_customer_session` | release `__Host-fiducia_admin_session` |
| Database | `fiducia-interfaces/sql/customer.sql` | `fiducia-interfaces/sql/admin.sql` |
| Authorization | verified user plus explicit org membership | trusted operator role plus local operator registry |
| Routes | `/app`, `/api/customer/*` | `/`, `/infra`, `/api/admin/*` |

Neither server reads the other database or accepts the other cookie. Cluster
locks, requests, KV, and service-discovery controls belong only to the admin
application; the customer portal does not expose cluster-wide operator data.

## Authentication and credentials

`POST /login` performs the email/password exchange from this Rust server to
Supabase Auth. The returned access token is verified through
`fiducia-auth GET /v1/me`; a customer cookie is issued only for a verified user
with trusted organization membership.

The token is stored in an `HttpOnly; SameSite=Strict; Secure` host-only cookie.
Release binaries use the browser-enforced `__Host-` prefix and ignore the local
plain-HTTP escape hatch. API clients may instead send the same token as
`Authorization: Bearer ...`; an explicit malformed or duplicate Authorization
header never falls back to an ambient cookie, and duplicate canonical cookies
are rejected. Browser JavaScript never receives a service-role key or the
application session token.

All dynamic customer responses are `no-store`. Browser writes require the exact
configured Host and Origin plus a credential-bound CSRF token. Login uses a
separate, short-lived host-only nonce cookie. WebSocket handshakes also require
the exact Origin, preventing a same-site sibling subdomain from borrowing the
ambient customer cookie.

`fiducia-auth` is the sole API-key authority. This BFF authenticates the customer,
requires an explicit verified organization for multi-org accounts, forwards
create/list/rotate/revoke operations and mutation `Idempotency-Key` values, and
returns only the typed sanitized display contract. It does not mint credentials,
store verifier hashes, or maintain a second credential database. Secret-bearing
responses are marked `Cache-Control: no-store`; exact retries are replay-safe in
the auth service.

## Routes

| Route | Purpose |
|---|---|
| `GET/POST /login` | server-mediated Supabase sign-in |
| `POST /logout` | clear only the customer cookie |
| `GET /app/*` | authenticated Maud customer pages |
| `GET /app/fragments/*` | authenticated HTMX fragments |
| `POST /app/api-keys` | replay-safe HTMX API-key creation; plaintext shown once |
| `POST /app/api-keys/rotate` | replay-safe HTMX rotation; replacement plaintext shown once |
| `POST /app/api-keys/revoke` | replay-safe HTMX revocation |
| `POST /app/settings` | SeaORM preference persistence |
| `POST /app/security/sessions/revoke` | user-scoped local session audit update |
| `GET /app/activity`, `GET /api/customer/activity` | verified-org, bounded audit activity; sensitive diagnostics excluded |
| `GET/POST /api/customer/*` | authenticated JSON customer BFF |
| `GET /app/ws`, `GET /app/events` | authenticated non-sensitive refresh heartbeat |
| `GET /healthz`, `GET /api/health` | health probes |
| `GET /api/info` | deployment metadata |
| `GET /docs/api`, `GET /api/docs.json` | generated human/machine-readable route inventory |
| other paths | static `fiducia-marketing.web` marketing build |

The heartbeat transports refresh signals and server-rendered summary fragments,
not customer rows, API-key metadata, or credentials. Customer data is reloaded
through authenticated, tenant-scoped routes.

## Database

`DATABASE_URL` is required and startup fails if Postgres is unavailable.
Production persistence uses one SeaORM `DatabaseConnection`. Supabase remains
the identity source of truth; SeaORM provisions the local user row and persists
customer preferences and user-scoped local session observations. Marking a local
session revoked is an audit-state change, not yet provider-backed Supabase token
revocation.

The schema source of truth is
[`fiducia-interfaces/sql/customer.sql`](../fiducia-interfaces/sql/customer.sql).
Real Postgres tests use `TEST_DATABASE_URL` when supplied and otherwise skip
without inventing database state.

### Declarative Supabase migrations

The operator workflow is [`scripts/dpm-schema.sh`](scripts/dpm-schema.sh), built
around [`dpm`](https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
It materializes the canonical SQL as the desired catalog, compares it with the
customer Supabase database, and uses a shadow database to prove convergence.
Use the Supabase direct connection (or session pooler on port 5432), never the
transaction pooler on port 6543.

```sh
cargo install --git https://github.com/declarative-migrations/declarative-postgres-migrate.rs --locked
DATABASE_URL=postgres://customer-target \
SHADOW_DATABASE_URL=postgres://scratch-admin \
scripts/dpm-schema.sh diff

DATABASE_URL=postgres://customer-target \
SHADOW_DATABASE_URL=postgres://scratch-admin \
scripts/dpm-schema.sh verify
```

`verify` does not write the target. Only after reviewed output has verified can
an operator set `DPM_APPLY_APPROVED=1` and run `scripts/dpm-schema.sh apply`.
The wrapper never opts into DPM's separate destructive-operation consent.

## Run locally

```sh
STATIC_DIR=../fiducia-marketing.web/dist \
DATABASE_URL=postgres://... \
FIDUCIA_AUTH_URL=http://127.0.0.1:8097 \
SUPABASE_URL=https://example.supabase.co \
SUPABASE_PUBLISHABLE_KEY=public-key \
CUSTOMER_APP_ORIGIN=http://127.0.0.1:8080 \
FIDUCIA_CUSTOMER_CSRF_SECRET=local-only-secret-at-least-32-bytes \
FIDUCIA_INSECURE_COOKIES=1 \
cargo run --locked
```

The server listens on `:8080` by default. `FIDUCIA_INSECURE_COOKIES=1` is honored
only by debug builds for local plain-HTTP development; release binaries always
emit `Secure` cookies.

| Variable | Meaning |
|---|---|
| `DATABASE_URL` | required customer Postgres credentials |
| `FIDUCIA_AUTH_URL` | required `fiducia-auth` base URL |
| `SUPABASE_URL` | required Supabase project URL |
| `SUPABASE_PUBLISHABLE_KEY` | required public key for server-mediated sign-in |
| `STATIC_DIR` | Astro marketing build; default `static` |
| `CUSTOMER_APP_HOST` | customer host; default `app.fiducia.cloud` |
| `CUSTOMER_APP_ORIGIN` | exact canonical customer origin and CORS allowlist; required and HTTPS in release |
| `FIDUCIA_CUSTOMER_CSRF_SECRET` | environment-only HMAC key, at least 32 bytes; required in release |
| `FIDUCIA_SITE_MODE=customer` | render the customer app at `/` regardless of Host |
| `PORT` | listen port; default `8080` |
| `FIDUCIA_INSECURE_COOKIES=1` | debug-only local escape hatch; ignored by release binaries |
| `TEST_DATABASE_URL` | opt-in real-Postgres behavior tests |

`FIDUCIA_E2E_STATIC_CUSTOMER_AUTH=1` exists only in debug builds. Release
binaries remain fail-closed.

Non-secret options can be mapped from audited flags:

```sh
make -B -C vendor/flags-2-env all
scripts/with-flags2env.sh --port 8080 --static-dir ../fiducia-marketing.web/dist -- cargo run --locked
```

### Reproducible container and CI dependency

The backend consumes generated Rust and database contracts from
`fiducia-interfaces` and security-critical webhook verification from
`fiducia-payments.rs`. CI and the Dockerfile pin those dependencies to commits
`bd718cd72d72aa330534f3688f8fb1ce90c19d10` and
`0c8b735949f90514545fd03dadcf3ba337a1e948`; neither follows a moving branch.
The container checks each commit out detached and verifies its full `HEAD`
before compiling with the committed Cargo lockfile. CI,
`Cargo.toml`, and the builder image use Rust 1.97; the registry-verified base and
distroless runtime are both pinned by digest.

The Docker workflow publishes `ghcr.io/fiducia-cloud/fiducia-backend` under the
exact source commit tag, with provenance and an SBOM. It contains the checked-in
static fallback and performs no source clone or asset build at pod startup.
Deployment is pull-based GitOps: the cluster configuration must provide
`CUSTOMER_APP_ORIGIN` and `FIDUCIA_CUSTOMER_CSRF_SECRET` through secret
management before the release binary can become ready; this repository never
receives cluster credentials or supplies production secret values.

To test another reviewed contract revision, pass its full commit id and update
the CI checkout pin in the same reviewed change:

```sh
docker build \
  --build-arg INTERFACES_SHA=<40-character-commit-sha> \
  -t fiducia-backend:local .
```

## Verification

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo audit
vendor/flags-2-env/build/flags2env audit .cli-flags.toml
git diff --check
```

`cargo audit` reads the narrow exceptions in `.cargo/audit.toml`:
`RUSTSEC-2023-0071` is an unreachable MySQL-only lockfile edge (this service
enables PostgreSQL only), while `RUSTSEC-2024-0370` and `RUSTSEC-2026-0173`
describe unmaintained proc-macro crates used only while compiling Maud and
SeaORM derives. They are not runtime accepts; retain the IDs only until upstream
releases remove those build-time dependencies, and recheck their reverse trees
on every dependency update.

<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a secondary git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/fiducia-customer.rs`; make changes here, not in that
> submodule checkout.
<!-- END k8s-cluster-submodule-notice -->

## Cross-surface delivery

User-visible, account, organization, API-key, session/activity, preference,
notification, permission, navigation, or deep-link changes in this Rust
customer web/BFF must be evaluated for:

- `fiducia-cloud/fiducia-flutter` on Android, iOS, Flutter Web/mobile web, and
  Flutter desktop;
- `fiducia-cloud/fiducia-desktop.rs`, the planned native GPUI desktop app; and
- Fiducia interfaces, generated clients, account/org/API-key/activity/session
  schemas, route types, auth fixtures, and conformance tests.

This is judgment-based coordination. Static marketing fallback, browser-only
account presentation, and server-only persistence/observability may remain
web-specific. Native secure storage, notifications, local diagnostics,
background operation, and OS integration may be native-specific. Account and
organization state, API-key lifecycle, session/activity semantics, preferences,
permissions, errors, notifications, and navigation normally require
coordinated updates or an explicit no-change rationale and parity follow-up.

The customer and admin planes remain separate. A customer-surface change must
not silently gain operator capabilities, accept the admin cookie, or depend on
the admin database.

Deep links are HTTPS-first:

```text
https://<verified-fiducia-owned-host>/open/<route>?<bounded-query>
```

with `fiducia://` fallback. Web, Flutter, and GPUI desktop must share versioned
route types and fixtures and support cold start, already-running delivery,
authentication resume, replay/expiry rejection, browser fallback, and explicit
confirmation before API-key creation/rotation/revocation, session revocation,
organization changes, or other security-sensitive actions.

API-key plaintext, verifier hashes, session tokens, cookies, organization
secrets, credentials, private activity records, bearer/refresh tokens, and
database identifiers are prohibited in URLs. Use bounded identifiers or
short-lived, single-use, audience-bound codes and validate route version,
user/org/key/session identity, action, authorization, assurance level, limits,
and user intent.

See [`docs/CROSS_SURFACE_DELIVERY.md`](docs/CROSS_SURFACE_DELIVERY.md) and the
[portfolio policy](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md).
