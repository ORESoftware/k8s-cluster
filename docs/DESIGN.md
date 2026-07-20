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

## Not yet built (roadmap)

- Refresh-token / session issuance (currently exchange is stateless, TTL-bounded).
- A shared client crate so downstream services verify our JWKS with a few lines instead of
  copy-pasted code — this is what actually retires the 15 duplicate verifiers.
- Optional Crossplane/`ExternalSecret`-driven per-project config sync from `discover`.
