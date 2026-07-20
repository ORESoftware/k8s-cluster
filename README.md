# shared-auth-server.rs

Centralized OreSoftware auth server. It verifies **Supabase** access tokens issued by any of
several projects (one per org — 3fa-app, athlet-o-store, fiducia-cloud, sonus-auris, …),
mirrors the verified identity into an AWS RDS `shared_auth` schema, and mints a single unified
OreSoftware JWT that every downstream service trusts via this server's published JWKS.

It runs **alongside** Supabase's built-in auth — a shortcut / parallel authority — not as a
replacement or a copy of Supabase's password store. Supabase authenticates; this server
verifies, indexes, and re-issues.

## Why

Supabase JWT verification is currently copy-pasted across ~15 services, inconsistently (some
on the deprecated HS256 shared-secret path, some on JWKS/RS256). This centralizes it: **one
verifier to audit, one key to rotate, one identity namespace** (`shared_user_id`) across all
orgs.

## What it is / isn't

- ✅ Verifies Supabase tokens from N projects (routes by `iss` → that project's JWKS).
- ✅ Mirrors identity (id, email, metadata) into `shared_auth.users` on RDS.
- ✅ Mints unified OreSoftware JWTs (ES256) and publishes `/.well-known/jwks.json`.
- ❌ Does **not** store passwords or mirror Supabase credentials.
- ❌ Does **not** use the account-level Supabase credential on the request path (see Security).

## HTTP API

Mounted under the cluster gateway at `/shared-auth/`.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/healthz` | liveness |
| `GET` | `/readyz` | readiness (DB ping if configured) |
| `GET` | `/.well-known/jwks.json` | our public JWKS — downstream verifiers fetch this |
| `POST` | `/auth/exchange` | Supabase access token → unified OreSoftware JWT |
| `POST` | `/auth/introspect` | validate an OreSoftware JWT → claims (RFC 7662 shape) |
| `GET` | `/auth/verify` | bearer check for the NGINX gateway `auth_request` |
| `GET` | `/metrics` | Prometheus |

### Exchange

```bash
curl -sS https://<gateway>/shared-auth/auth/exchange \
  -H "Authorization: Bearer <supabase_access_token>"
# → { "access_token": "<ore_jwt>", "token_type": "Bearer",
#     "expires_at": 1753000000, "shared_user_id": "…", "project": "fiducia-cloud" }
```

## Configuration (environment)

| Var | Required | Meaning |
|---|---|---|
| `AUTH_SUPABASE_PROJECTS` | ✅ | JSON array of projects — see below |
| `AUTH_SIGNING_KEY_PEM` / `AUTH_SIGNING_KEY_FILE` | ✅ | PKCS#8 EC P-256 private key (ES256) |
| `AUTH_SIGNING_KID` | | `kid` advertised in our JWKS (default `shared-auth-v1`) |
| `AUTH_ISSUER` / `AUTH_AUDIENCE` | | claims on minted tokens |
| `AUTH_TOKEN_TTL_SECS` | | minted-token lifetime (default 3600) |
| `AUTH_DATABASE_URL` | | RDS DSN (`search_path=shared_auth`); omit to disable mirroring |
| `AUTH_BIND_ADDR` | | default `0.0.0.0:8120` |
| `AUTH_CORS_ALLOW_ORIGINS` | | comma-separated browser origins |

`AUTH_SUPABASE_PROJECTS`:

```json
[
  { "name": "fiducia-cloud",  "project_ref": "abcdefghijklmnopqrst" },
  { "name": "3fa-app",        "project_ref": "uvwxyz0123456789abcd" },
  { "name": "athlet-o-store", "project_ref": "…" },
  { "name": "sonus-auris",    "project_ref": "…" }
]
```

`issuer` (`https://<ref>.supabase.co/auth/v1`) and `jwks_url` are derived from `project_ref`.
Set `hs256_secret` on a project only if it is still on legacy shared-secret signing.

### Discovering projects

Enumerate the account's orgs/projects and print a ready-to-paste config:

```bash
SUPABASE_ACCESS_TOKEN=sbp_… shared-auth-server discover
```

This is the **only** use of the account-level PAT and it never runs in the serving process.

## Generate a signing key

```bash
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out signing.pem
# load into the cluster secret store as AUTH_SIGNING_KEY_PEM (never commit it)
```

## Security model

- **Account credential off the hot path.** The Supabase PAT for
  alexander.d.mills@gmail.com can delete projects; it is used only by `discover`. The serving
  process holds only public JWKS URLs and our own signing key.
- **Identity mirror, not credential mirror.** `shared_auth.users` holds ids/emails/metadata —
  never passwords, never the service-role key.
- **Uniform rejection.** Every auth failure returns `401 unauthorized`, so a caller cannot
  probe which projects or users exist.
- **Bounded JWKS fetches.** Per-project cache (10 min) + single-flight + 30s refresh floor, so
  an unknown-`kid` flood cannot amplify into outbound requests.

## Deploy

`deploy/k8s/` holds the namespace-scoped manifests (Deployment/Service/ExternalSecret/
NetworkPolicy) for the `shared-auth` tenant. The platform (ORESoftware/k8s-cluster) owns the
Namespace + AppProject and registers this repo directly with ArgoCD. See
`docs/DESIGN.md` and the k8s-cluster `docs/app-deploy-contract.md`.

## Layout

```
src/
  main.rs            thin entrypoint
  lib.rs             run(): dispatch serve | discover
  config.rs          env-driven config
  supabase/          per-project JWKS verification, issuer routing, Management API
  db/                RDS identity mirror (shared_auth.users)
  token/             mint unified JWT + publish our JWKS
  http/              axum routes
db/schema.sql        declarative shared_auth schema (owned by pg-defs/dpm)
deploy/k8s/          tenant manifests
```
