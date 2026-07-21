# Design — shared-auth-server

## Problem

Every OreSoftware service that accepts Supabase-authenticated users re-implements Supabase
JWT verification. Today ~15 services do this, and inconsistently: some verify HS256 with the
project's shared secret (the path Supabase is deprecating), some verify JWKS/RS256 with key
rotation. That is N copies of a security boundary to audit and N places a Supabase key change
can break.

## Approach: verifier + re-issuer, not a mirror

Supabase stays the identity provider. This server sits in front as a **verifier and token
authority**:

```
                      ┌────────────────────── shared-auth-server ──────────────────────┐
 Supabase token  ──►  │  registry.route(iss) → project verifier → JWKS verify           │
 (any project)        │        │                                                        │
                      │        ▼                                                        │
                      │  mirror identity → shared_auth.users (RDS)  →  shared_user_id    │
                      │        │                                                        │
                      │        ▼                                                        │
                      │  mint ES256 OreSoftware JWT (sub = shared_user_id)               │
                      └────────┬───────────────────────────────────────────────────────┘
                               ▼
        downstream services verify against /.well-known/jwks.json  (one key, one verifier)
```

The unified token's `sub` is a stable `shared_user_id` from the mirror, so downstream services
get **one identity namespace** regardless of which Supabase project (org) the user came from.

## Module boundaries

| Module | Responsibility |
|---|---|
| `config` | env → `AppConfig` (projects, signing key, DB, bind) |
| `supabase::verifier` | one project: JWKS cache, single-flight refresh, verify |
| `supabase::registry` | route a token to its project by (unverified) `iss`, then verify |
| `supabase::management` | offline `discover` only — the account PAT lives here and nowhere else |
| `db` | `shared_auth.users` upsert/read (no DDL; schema owned by pg-defs) |
| `token::minter` | sign ES256 OreSoftware JWTs, and verify our own |
| `token::jwks` | derive our public JWKS from the signing key |
| `http` | axum routes; each endpoint is one file |
| `state` | `AppState` wiring, cloned per request |

`main.rs` is a shell; all logic is in the library so it is unit-testable and the boundaries
above are real module boundaries, not one file.

## Multi-project routing

A token's `iss` is untrusted, but it is safe to route on: we pick the verifier whose configured
issuer equals the token's `iss`, and that verifier re-pins `iss` during real verification. A
forged `iss` therefore only ever selects a verifier that will reject the signature. Unknown or
missing issuer is `401`, indistinguishable from a bad signature.

## Threat model / decisions

- **Account PAT off the hot path.** The Supabase management token
  (alexander.d.mills@gmail.com) can delete projects. It is confined to the `discover`
  subcommand; the serving process only ever holds public JWKS URLs + our own signing key.
- **Identity, not credentials.** The RDS mirror stores ids/emails/metadata for downstream
  authorization and traceability. No passwords; the Supabase service-role key is never used.
- **Uniform failures.** Every rejection is `401 unauthorized`. No endpoint reveals whether a
  project or user exists.
- **Amplification bounds.** Per-project JWKS cache (10 min, matching Supabase's edge), a 30s
  refresh floor, and a single-flight lock, so a flood of unknown-`kid` tokens cannot become one
  outbound fetch per token.
- **Email gate is confirmation-gated.** `email_verified` is read from both the top-level claim
  and legacy `user_metadata`; an unconfirmed address is not treated as evidence of identity.
- **Key rotation.** `kid` is stable per key. To rotate, publish old+new in the JWKS during an
  overlap window, then retire the old `kid`.

## Tandem resilience — either side can carry a short outage

The server is designed to run *in tandem* with Supabase, so a multi-minute outage
of either does not break auth:

- **Supabase down → shared-auth keeps verifying.** The per-project JWKS cache has
  a soft TTL (10 min) and a longer **grace window** (60 min). Past the soft TTL we
  try to refresh; if Supabase is unreachable and the key is still within grace, we
  serve the *stale-but-recent* key rather than failing. JWKS keys rotate rarely, so
  a token signed by a key we fetched minutes ago still verifies through the outage.
  (A brand-new `kid` we have never fetched is still rejected — we cannot verify a
  key we never saw.) Tested in `supabase::verifier::tests`.
- **shared-auth down → the OreSoftware token still works, and Supabase still works
  directly.** A minted OreSoftware token is verified against *our own* JWKS with no
  Supabase call, so downstream services keep accepting live sessions for the token
  TTL even if this server is down. And because Supabase remains the identity
  provider, a service can always fall back to verifying a raw Supabase token
  directly (the pre-consolidation path) until shared-auth returns.
- **At the edge:** the Cloudflare Worker (`cloudflare/`) verifies OreSoftware
  sessions locally against a cached JWKS (rides out a shared-auth blip) and, as a
  fallback, exchanges a Supabase token for an OreSoftware one (rides out a login-
  time shared-auth blip). Redirects to login only when neither path yields a
  session.

The failover is asymmetric and deliberate: **verification never has a hard
dependency on a live remote**, only *minting/exchange* does. That is what lets a
valid session survive either side going away.

## Observability

Explicit, no agents (vendored from the org's `dd-telemetry`):
- **Traces:** `tracing` spans → OTLP/HTTP → `dd-otel-collector.observability:4318`
  → Tempo/Jaeger. One span per request via `http_trace_layer()`, W3C traceparent
  propagated.
- **Logs:** structured JSON on stdout → promtail → Loki, carrying `trace_id`.
- **Metrics:** a dedicated Prometheus registry at `/metrics`
  (`shared_auth_exchanges_total`, `_verify_failures_total`,
  `_introspections_total`) → Prometheus → Grafana.

## UI (MASH)

Maud + htmx + (Axum) + SeaORM + Supabase. HTML is served only where a human needs
it — a status landing page and a token-exchange helper whose form posts to an htmx
endpoint that swaps a result fragment. The auth API itself is JSON. No websockets.

## Config & flags

`.cli-flags.toml` (flags-2-env) maps non-secret flags to `AUTH_*` env vars; secrets
(signing key, DB URL, project list, PAT) are environment-only and excluded from
flags. Applied at startup before config is read (`src/flags.rs`), best-effort.

## Deploy flow (GitOps, rolling)

CI (`.github/workflows/release.yml`) builds the image, pushes it, and **pins the
immutable digest into `deploy/k8s/deployment.yaml`**. ArgoCD tracks this repo's
`deploy/k8s` directly, so that pin commit is the deploy trigger and rolls the
Deployment with `maxUnavailable: 0, maxSurge: 1` (+ a PDB) for zero downtime. The
k8s-cluster submodule pin is refreshed separately as *inventory only* and never
triggers a redeploy. Blue/green is available by fronting the Deployment with an
Argo Rollouts `Rollout` (the app is stateless, so rolling or blue/green both fit).

## Merging fiducia-auth (planned)

`fiducia-auth.rs` will fold into this server. It currently adds, on top of what we
have: **API-key issuance/introspection** backed by Fiducia KV (create/list/revoke,
`POST /v1/introspect` with a positive-introspection cache), and fiducia-specific
JWT signing. The merge path:
1. Generalize the project registry to also resolve **API-key** credentials, not
   just Supabase JWTs — an API key introspects to the same `VerifiedIdentity`.
2. Add a pluggable key-store behind a trait (RDS mirror here; Fiducia KV for the
   fiducia tenant) so introspection is source-agnostic.
3. Keep fiducia's `/v1/introspect` contract as an alias of `/auth/introspect` so
   the fiducia edge/LB cache keeps working during cutover.
4. Retire `fiducia-auth.rs` once its tenants read from the shared server. Its
   `.cli-flags.toml` shape and secret-exclusion list are already the model this
   repo follows.

## Not yet built (roadmap)

- Refresh-token / session issuance (currently exchange is stateless, TTL-bounded).
- A shared client crate so downstream services verify our JWKS with a few lines instead of
  copy-pasted code — this is what actually retires the 15 duplicate verifiers.
- Optional Crossplane/`ExternalSecret`-driven per-project config sync from `discover`.
