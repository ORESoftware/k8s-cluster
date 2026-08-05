# shared-auth-server.rs

Development starts with `nix develop ./.nix`. Non-secret CLI options are
declared in `.cli-flags.toml` and parsed by
[`flags-2-env`](https://github.com/oresoftware/flags-2-env) before configuration
is loaded. Runtime logs are structured and spans use OTLP/HTTP OpenTelemetry.

Rust authentication authority for ORESoftware services. The same binary is
deployed independently as `shared-auth-admin` and `shared-auth-customer`.
Each deployment has its own issuer, Supabase project, PostgreSQL RDS endpoint,
signing key, cookie namespace, secret paths, session store, and recovery policy.
The two deployments share source code but never runtime authority.

Postgres is the source of truth for principals, provider links, Argon2id
credentials, coarse Shared Auth roles, application enrollment, and refresh
sessions. Supabase Auth remains an upstream credential authority: a verified
realm-specific Supabase access token can be exchanged for the Shared Auth
session used by that realm. A normal DB-backed production deployment accepts
exactly one dedicated Supabase project. An empty provider registry is accepted
only by the explicit DB-less or all-loopback development/test modes.

The storage model is provider-neutral. Supabase is the first external adapter;
Clerk, Cognito, or another OIDC provider can be added by writing a verifier that
produces the existing `AuthenticatedIdentity` shape.

See [`docs/runtime-realm-contract.md`](docs/runtime-realm-contract.md) for the
executable deployment contract and
[`docs/auth-realms-and-federated-sso.md`](docs/auth-realms-and-federated-sso.md)
for the complete architecture and rollout rationale.

## Security properties

- Local passwords are Argon2id PHC hashes; plaintext passwords never enter the
  database or logs.
- Refresh tokens contain 256 bits of randomness, are stored only as SHA-256
  hashes, and rotate atomically. Replaying a consumed token is rejected.
- Magic-link tokens contain 256 bits of randomness. Link tokens and six-digit
  email OTPs are stored only as hashes, expire together, and are consumed once.
- Email delivery uses SendGrid's HTTPS API. SMS second-factor challenges use
  Twilio Verify, so shared-auth never stores an SMS verification code.
- Access tokens are short-lived ES256 JWTs with issuer, audience, expiry,
  not-before, unique token id, session id, provider provenance, roles,
  authentication assurance level (`aal`), and authentication methods (`amr`).
- Admin and customer startup fails closed when deployment identity, issuer,
  exact database endpoint host/resource reference, secret path, signing-key
  reference, cookie name, or Supabase project does not match the selected realm.
- The application database is not an authentication fallback. A non-empty
  `AUTH_APPLICATION_DATABASE_URL` is rejected by the server.
- Customer SSO reuses a central login ceremony, never another application's
  bearer token or cookie. Delegated product tokens carry an exact target
  audience, authorized client, bounded scopes, session, and assurance lineage.
- Product organizations, memberships, billing grants, and resource permissions
  remain authoritative in each product database.
- Postgres is authoritative. Redis/Valkey is optional and only accelerates
  rate-limit and revocation checks.
- External identities are never linked merely because their email addresses
  match. Provider, tenant, and provider subject form the external identity key.
- The provider-sync endpoint requires a timestamped HMAC signature and records
  event ids for cross-replica idempotency.
- JWKS verification pins issuer/audience and accepts only ES256/RS256 for the
  asymmetric Supabase path. Fetches are bounded, redirect-free, single-flight,
  and stale-within-grace during provider outages.
- Request bodies, provider tokens, identifiers, CORS origins, and session
  lifetimes are bounded.

## HTTP API

The cluster gateway mounts the service at `/shared-auth/`; paths below are the
service-local paths.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/healthz` | Process liveness |
| `GET` | `/readyz` | Postgres readiness |
| `GET` | `/.well-known/jwks.json` | Public ES256 JWKS |
| `POST` | `/auth/register` | Create a local account when registration is enabled |
| `POST` | `/auth/login` | Local email/password login |
| `POST` | `/auth/passwordless/request` | Send a SendGrid magic link and six-digit email OTP |
| `POST` | `/auth/passwordless/consume` | Consume a link token or email plus OTP |
| `POST` | `/auth/mfa/sms/request` | Start a Twilio Verify SMS challenge for a signed-in user |
| `POST` | `/auth/mfa/sms/verify` | Verify SMS and issue a new AAL2 session |
| `POST` | `/auth/refresh` | Rotate a refresh token and issue a new token pair |
| `POST` | `/auth/logout` | Revoke a refresh session |
| `POST` | `/auth/exchange` | Realm Supabase access token to Shared Auth token pair |
| `POST` | `/auth/delegate` | Exchange a base customer token for an allow-listed audience/scope token |
| `POST` | `/auth/introspect` | RFC 7662-shaped access-token check |
| `GET` | `/auth/verify` | Gateway `auth_request` target |
| `POST` | `/internal/webhook/sync` | HMAC-authenticated provider/session/role sync |
| `GET` | `/metrics` | Prometheus exposition |

Local auth request bodies:

```json
{ "email": "person@example.com", "password": "a long passphrase" }
```

Registration accepts an optional `display_name`. Login, registration, and
Supabase exchange return an access token plus a one-time refresh token. Refresh
tokens must be sent only to `/auth/refresh` or `/auth/logout`, never as bearer
tokens to application services.

Passwordless email:

```json
{ "email": "person@example.com" }
```

`POST /auth/passwordless/request` always returns `202` for a syntactically valid
address, regardless of account existence. Consume either the link token:

```json
{ "token": "sat_magic_..." }
```

or the six-digit code:

```json
{ "email": "person@example.com", "otp": "123456" }
```

SMS MFA endpoints require the current Shared Auth access token as a bearer
token. Enroll or challenge with an E.164 phone number, then submit the returned
`challenge_id` with the Twilio code:

```json
{ "phone": "+14155550100" }
```

```json
{ "challenge_id": "00000000-0000-0000-0000-000000000000", "code": "123456" }
```

## Configuration

| Variable | Required | Meaning |
|---|---:|---|
| `AUTH_REALM` | DB-backed | Exact realm: `admin` or `customer` |
| `AUTH_REALM_DEPLOYMENT` | DB-backed | Deployment identity that visibly names the realm |
| `AUTH_DATABASE_ENDPOINT_HOST` | DB-backed | Exact expected PostgreSQL host; the host parsed from `AUTH_DATABASE_URL` must match it |
| `AUTH_DATABASE_RESOURCE_REF` | DB-backed | Non-secret realm-specific RDS resource reference |
| `AUTH_DATABASE_SECRET_REF` | DB-backed | Non-secret realm-specific secret-manager path for the DSN |
| `AUTH_SIGNING_KEY_REF` | DB-backed | Non-secret realm-specific signing-key secret path |
| `AUTH_SESSION_COOKIE_NAME` | DB-backed | Realm-specific `__Host-` cookie name |
| `AUTH_REALM_SUPABASE_PROJECT_REF` | DB-backed | Dedicated Supabase project ref expected for the realm |
| `AUTH_DATABASE_URL` | DB-backed | Realm PostgreSQL DSN; schema is `shared_auth` |
| `AUTH_SIGNING_KEY_PEM` or `AUTH_SIGNING_KEY_FILE` | yes | PKCS#8 P-256 private key |
| `AUTH_SUPABASE_PROJECTS` | production | JSON array containing exactly the selected realm project |
| provider credential env vars | per project | Publishable, secret/service-role, or legacy JWT keys referenced by name from project metadata |
| `AUTH_REALM_ALLOW_LOOPBACK` | no | Explicit all-loopback development/test escape hatch; default `false` |
| `AUTH_SENDGRID_API_KEY` | for email | SendGrid API key; secret and never accepted as a CLI flag |
| `AUTH_OTP_PEPPER` | for email | At least 32 bytes used to HMAC email OTPs; secret |
| `AUTH_EMAIL_FROM` | for email | Verified SendGrid sender address; empty disables passwordless email |
| `AUTH_EMAIL_FROM_NAME` | no | Sender display name; default `OreSoftware` |
| `AUTH_MAGIC_LINK_BASE_URL` | for email | HTTPS or app deep-link URL; empty disables passwordless email |
| `AUTH_MAGIC_LINK_TTL_SECS` | no | Link and email OTP lifetime, 300–3600; default 900 |
| `AUTH_MAGIC_LINK_ALLOW_SIGNUP` | no | Let a verified email create a principal; default `false` |
| `AUTH_TWILIO_ACCOUNT_SID` | for SMS MFA | Twilio account SID; secret |
| `AUTH_TWILIO_AUTH_TOKEN` | for SMS MFA | Twilio auth token; secret |
| `AUTH_TWILIO_VERIFY_SERVICE_SID` | for SMS MFA | Twilio Verify service SID; secret |
| `AUTH_REDIS_URL` | no | Private Redis/Valkey URL |
| `AUTH_WEBHOOK_SECRET` | no | 32+ byte HMAC secret; unset disables sync |
| `AUTH_ALLOW_REGISTRATION` | no | Public local registration; default `false` |
| `AUTH_ACCESS_TOKEN_TTL_SECS` | no | Access TTL, 60–86400; default 900 |
| `AUTH_REFRESH_TOKEN_TTL_SECS` | no | Refresh TTL, 300–31536000; default 2592000 |
| `AUTH_ISSUER` / `AUTH_AUDIENCE` | yes | Realm JWT constraints; admin must use an `admin-auth` host |
| `AUTH_CORS_ALLOW_ORIGINS` | no | Comma-separated exact browser origins |
| `AUTH_ALLOW_DBLESS` | no | Explicit development/test escape hatch only |

Example customer-realm provider registry:

```json
[
  {
    "name": "shared-auth-customer",
    "project_ref": "abcdefghijklmnopqrst",
    "publishable_key_env": "AUTH_SUPABASE_CUSTOMER_PUBLISHABLE_KEY",
    "secret_key_env": "AUTH_SUPABASE_CUSTOMER_SECRET_KEY"
  }
]
```

The admin process receives a separate one-element registry with a different
project ref and credentials. Every `*_env` value names a separate process
environment variable; the parser rejects inline key material and fails startup
if a referenced variable is absent. Modern asymmetric JWT verification needs
no API key, so omit unused references and grant each configured key the
narrowest Supabase scope available. Use `shared-auth-server discover` with
`SUPABASE_ACCESS_TOKEN` only from an operator workstation or a short-lived
Fiducia-injected job. The account token is never injected into or used by a
serving deployment.

SendGrid and Twilio are optional integrations. Leaving all of their variables
unset keeps their endpoints disabled without affecting server startup,
password login, Supabase exchange, refresh, introspection, or application
traffic. Once any credential in an integration is supplied, its companion
variables must also be valid so partial secret rollouts fail clearly.

## Customer application federation

`db/schema.sql` defines a stable global principal plus per-application records:
`applications`, `application_accounts`, `oauth_clients`,
`application_consents`, and `session_application_grants`. A customer may be
active in App A and suspended or unenrolled in App B without changing the
global principal.

The customer realm can reuse its central login session when a customer opens
another first-party application, but it creates or validates that application's
account and issues a new token for that application's exact audience and client.
App A must reject App B's token and vice versa. Neither application receives the
other application's cookie, bearer token, roles, organization memberships, or
database access.

## Database and deployment

`db/schema.sql` is a reviewable copy of the declarative schema. The canonical
RDS contract is also kept in the cluster's `pg-defs` repository and migrations
must be generated/reviewed with `dpm`; the application never executes DDL.

The target production topology uses separate admin-auth and customer-auth RDS
instances. The exact RDS endpoint host exported by infrastructure is supplied as
`AUTH_DATABASE_ENDPOINT_HOST` and must match the host in the protected DSN. The
legacy single-realm Kubernetes resources under `deploy/k8s/` are not silently
rewritten by the realm implementation; reviewed overlays and the new
realm-specific secrets must exist before cutover. This prevents a watched
manifest from switching to nonexistent databases or secret paths.

Kubernetes deployments consume RDS, Redis, signing, provider, and webhook values
through External Secrets. Logs are structured JSON to stdout for Promtail/Loki,
traces use OTLP/HTTP to the cluster collector, and `/metrics` is scraped by
Prometheus for Grafana.

The Cloudflare Worker and AWS RDS definitions live in `shared-auth-infra`, not
this application repo.

## Development

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

The browser integration suite uses the explicit all-loopback customer test
profile. Exercise registration, refresh, revocation, App-A/App-B audience
rejection, and admin/customer rejection against disposable Postgres instances
before schema or traffic promotion.
