# shared-auth design

## Authority model

Postgres-backed shared-auth is the primary authority. Supabase Auth is a
secondary, independently usable authority. A user can arrive through either
path and receives a shared-auth access/refresh session with provider provenance.

```text
local password ── Argon2id verify ─┐
                                   ├─ shared user + roles ─ session ─ ES256 JWT
Supabase JWT ── issuer/JWKS verify ┘
```

The server does not copy Supabase passwords or refresh tokens. It stores its own
sessions and a link from `(provider, provider_tenant, provider_subject)` to a
stable shared user. Additional adapters must end at this same boundary.

## Data ownership

- `principals`: stable shared identity and lifecycle state.
- `provider_identities`: external subject links and provider metadata.
- `local_credentials`: Argon2id hashes and lockout state.
- `roles`: current authorization grants.
- `sessions`: hashed rotating refresh tokens and revocation state.
- `webhook_events`: replay/idempotency ledger for signed sync events.

The schema is declarative and applied outside the service. No Rust startup path,
handler, or entity generates DDL.

## Session lifecycle

1. Local login or external exchange resolves an `AuthenticatedIdentity`.
2. The server generates a 256-bit opaque refresh token and stores only its
   SHA-256 digest in Postgres.
3. A short-lived ES256 access token carries `sid`, provider provenance, and
   roles.
4. Refresh locks and revokes the old session row before inserting the successor.
   Concurrent replay of the old token fails.
5. Logout or a signed sync event revokes the Postgres row and writes a Redis
   revocation hint. Postgres remains the final check.

Downstream services may verify JWT signatures offline for availability. Services
that need immediate revocation use `/auth/verify` or `/auth/introspect`; otherwise
the access-token TTL is the maximum revocation delay.

## Provider failover

Supabase JWKS keys have an in-process soft TTL and a longer grace window. If the
provider is briefly unreachable, known keys inside the grace window continue to
verify. Unknown keys never pass. Shared-auth access tokens verify against the
server's public JWKS without a Supabase call.

The reusable Rust guard in `shared-auth-lib` races authorities and distinguishes
an invalid credential from an unavailable authority. Privileged actions fail
closed when no authority can decide.

## Redis posture

Redis/Valkey stores only bounded counters and revocation hints under a versioned
prefix. No password, raw email, access token, or refresh token is cached. Cache
loss causes DB fallback; it cannot resurrect a revoked Postgres session.

## Provider sync

`/internal/webhook/sync` accepts a timestamp (five-minute skew), a base64url
HMAC-SHA256 signature over `timestamp.raw_body`, and a UUID event id. Supported
operations are provider identity upsert, role replacement, and session
revocation. Each operation is idempotent; the event ledger makes retries visible
across replicas.

## Observability

- Rust `tracing` emits structured JSON to stdout for Loki.
- Explicit spans export through OTLP/HTTP to the cluster collector.
- Prometheus counters cover exchanges, verification failures, and introspection.
- Provider subjects, password material, and tokens are never log fields.

## Deployment boundary

The application repo owns only namespace-scoped workload resources. The cluster
repo owns the namespace, quota, default deny, AppProject, and direct ArgoCD
registration. The shared-auth monorepo and k8s-cluster submodules are inventory
pins, never Argo render sources.

Provider metadata and credentials are deliberately separate. The project registry
contains only issuer/audience information and environment-variable names. Fiducia
projects the named Supabase publishable, secret/service-role, or legacy JWT keys
through the cluster `ExternalSecret`; the server resolves them at startup without
serializing or logging their values. Future Clerk or Cognito adapters must use the
same reference-by-environment pattern instead of adding credentials to provider
JSON, command-line flags, images, Workers, or traces.
