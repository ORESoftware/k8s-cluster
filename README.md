# shared-auth-server.rs

Development starts with `nix develop ./.nix`. Non-secret CLI options are
declared in `.cli-flags.toml` and parsed by
[`flags-2-env`](https://github.com/oresoftware/flags-2-env) before configuration
is loaded. Runtime logs are structured and spans use OTLP/HTTP OpenTelemetry.

Rust authentication authority for ORESoftware services. Postgres is the source
of truth for users, provider links, Argon2id credentials, roles, and refresh
sessions. Supabase Auth remains available as a secondary authority: a verified
Supabase access token can be exchanged for the same shared-auth session used by
local accounts. An empty `AUTH_SUPABASE_PROJECTS` registry is valid, so a
Supabase outage or an intentionally unconfigured project does not prevent
Postgres-backed login, refresh, introspection, or existing application sessions.

The storage model is provider-neutral. Supabase is the first external adapter;
Clerk, Cognito, or another OIDC provider can be added by writing a verifier that
produces the existing `AuthenticatedIdentity` shape.

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
| `POST` | `/auth/exchange` | Supabase access token to shared-auth token pair |
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

SMS MFA endpoints require the current shared-auth access token as a bearer
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
| `AUTH_DATABASE_URL` | yes | RDS Postgres DSN; schema is `shared_auth` |
| `AUTH_SIGNING_KEY_PEM` or `AUTH_SIGNING_KEY_FILE` | yes | PKCS#8 P-256 private key |
| `AUTH_SUPABASE_PROJECTS` | no | JSON provider metadata and names of credential env vars; default `[]` |
| provider credential env vars | per project | Publishable, secret/service-role, or legacy JWT keys referenced by name from project metadata |
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
| `AUTH_ISSUER` / `AUTH_AUDIENCE` | no | Shared-auth JWT constraints |
| `AUTH_CORS_ALLOW_ORIGINS` | no | Comma-separated exact browser origins |
| `AUTH_ALLOW_DBLESS` | no | Explicit development/test escape hatch only |

Example provider registry:

```json
[
  {
    "name": "fiducia-cloud",
    "project_ref": "abcdefghijklmnopqrst",
    "publishable_key_env": "AUTH_SUPABASE_FIDUCIA_PUBLISHABLE_KEY",
    "secret_key_env": "AUTH_SUPABASE_FIDUCIA_SECRET_KEY"
  },
  {
    "name": "threefa",
    "project_ref": "uvwxyz0123456789abcd",
    "service_role_key_env": "AUTH_SUPABASE_THREEFA_SERVICE_ROLE_KEY",
    "jwt_secret_env": "AUTH_SUPABASE_THREEFA_JWT_SECRET"
  }
]
```

Every `*_env` value names a separate process environment variable; the parser
rejects inline key material and fails startup if a referenced variable is absent.
Modern asymmetric JWT verification needs no API key, so omit unused references
and grant each configured key the narrowest Supabase scope available. Use
`shared-auth-server discover` with `SUPABASE_ACCESS_TOKEN` only from an operator
workstation or a short-lived Fiducia-injected job. The account token is never
injected into or used by the serving Deployment.

SendGrid and Twilio are optional integrations. Leaving all of their variables
unset keeps their endpoints disabled without affecting server startup,
password login, Supabase exchange, refresh, introspection, or application
traffic. Once any credential in an integration is supplied, its companion
variables must also be valid so partial secret rollouts fail clearly.

## Database and deployment

`db/schema.sql` is a reviewable copy of the declarative schema. The canonical
RDS contract is also kept in the cluster's `pg-defs` repository and migrations
must be generated/reviewed with `dpm`; the application never executes DDL.

Kubernetes resources under `deploy/k8s/` are namespace-scoped and consume RDS,
Redis, signing, provider, and webhook values through External Secrets. The
`dd/shared-auth/provider-credentials` secret object is extracted into environment
variables whose names match the provider registry; Fiducia can manage and rotate
that object without committing values or rebuilding an image. Logs are
structured JSON to stdout for Promtail/Loki, traces use OTLP/HTTP to the cluster
collector, and `/metrics` is scraped by Prometheus for Grafana.

The Cloudflare Worker lives in `shared-auth-infra`, not this application repo.

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

The integration suite uses the explicit DB-less test configuration. Exercise
registration/refresh/revocation against a disposable Postgres instance before a
schema promotion.
