# Canonical Plus authenticated quote rollout

## Goal

Serve the signed-in quote workspace at:

```text
https://app.canonical.plus/u/quote
```

The public marketing site links directly to that URL. Anonymous users are sent
through the same-origin Shared Auth browser ceremony and returned only to the
sealed relative path `/u/quote`.

## Components and ownership

| Boundary | Repository | Responsibility |
|---|---|---|
| Public CTA | `canonical-cloud/canonical-marketing-site.web` | Sign-in and “Get a quote · under 5 min” links |
| Quote web/API | `canonical-cloud/canonical-web-server.rs` | Maud + Axum UI, REST, WebSockets, SeaORM persistence, Gemini analysis |
| Browser identity authority | `shared-auth/shared-auth-server.rs` | Magic-link/OTP ceremony, host-only access cookie, JWT verification/revocation |
| Edge enforcement | `shared-auth/shared-auth-infra` | `app.canonical.plus` protected-prefix routing and sanitized verified identity headers |
| GitOps | `ORESoftware/k8s-cluster` | ArgoCD project/application and tenant boundary |
| Product data plane | Canonical Supabase + RDS | Independent Canonical identity/session/context/quote records |

## Trust flow

1. The browser requests `/u/quote`.
2. The Cloudflare Worker may use cookie presence as a routing hint, but it never
   treats presence as authorization.
3. Without a valid browser session, the Worker or Rust origin redirects to:

   ```text
   /shared-auth/auth/browser/sign-in?return=%2Fu%2Fquote
   ```

4. Shared Auth accepts only a bounded relative return path, completes the
   Canonical Supabase ceremony, and issues the host-only
   `__Host-canonical-customer-auth` cookie for `app.canonical.plus`.
5. The Worker verifies the access token, strips caller-supplied `x-auth-*`
   headers, and forwards sanitized verified identity metadata.
6. The Rust origin independently sends the raw cookie token to the Canonical
   Shared Auth `/auth/verify` endpoint. It ignores edge identity headers for its
   authorization decision.
7. The origin establishes the quote principal, derives a token-bound CSRF
   value, and executes owner-scoped PostgreSQL transactions under forced RLS.

An invalid higher-precedence credential never falls back to another transport:
explicit bearer, existing Canonical app session, then Shared Auth browser cookie.

## Realm isolation

Canonical Plus runs the common Shared Auth binary as an independent authority:

- issuer: `https://app.canonical.plus/shared-auth`
- audience: `canonical-plus-web`
- cookie: `__Host-canonical-customer-auth`
- independent Supabase project
- independent PostgreSQL identity/session schema and credentials
- independent signing key
- independent browser return-state sealing key
- independent Redis namespace/endpoint

Do not share these values with `fiducia.cloud`, `oresoftware.com`, or the generic
Shared Auth deployment. Cross-product SSO, when desired, should be an explicit
federation/grant protocol rather than shared cookies or shared database rows.

## Quote analysis boundary

The quote service combines:

1. version-controlled `context/compliance-quote.md`;
2. the latest active PostgreSQL `canonical_context` record with key
   `quote-analysis`; and
3. the authenticated customer's validated intake.

Gemini receives bounded context and a structured response schema. Customer and
context fields are treated as untrusted data, not instructions. The result is a
preliminary scope/timeline/investment range for human review, never an audit
opinion, certification, attestation, or legal conclusion.

PostgreSQL remains authoritative. REST reads recover current status; WebSocket
messages are notification hints. The asynchronous states are `queued`,
`analyzing`, `ready`, and `failed`.

## Required secrets

The Canonical overlay reads only these External Secrets paths:

```text
dd/shared-auth/customer/canonical-plus/supabase-projects
dd/shared-auth/customer/canonical-plus/provider-credentials
dd/shared-auth/customer/canonical-plus/signing-key-pem
dd/shared-auth/customer/canonical-plus/database-url
dd/shared-auth/customer/canonical-plus/database-endpoint-host
dd/shared-auth/customer/canonical-plus/database-resource-ref
dd/shared-auth/customer/canonical-plus/supabase-project-ref
dd/shared-auth/customer/canonical-plus/redis-url
dd/shared-auth/customer/canonical-plus/webhook-secret
dd/shared-auth/customer/canonical-plus/introspect-secret
dd/shared-auth/customer/canonical-plus/browser-seal-secret
```

The quote runtime additionally requires `GEMINI_API_KEY`; `GEMINI_MODEL`
defaults to `gemini-2.5-pro` and `QUOTE_ANALYSIS_MAX_CONCURRENCY` defaults to 4.

## Ordered rollout

1. Merge and independently validate the Shared Auth browser-session PR.
2. Merge and independently validate the Cloudflare Worker PR.
3. Merge the Canonical quote web/API PR after its full permanent CI matrix is green.
4. Apply the Canonical desired PostgreSQL schema with the privileged migration identity.
5. Provision the exact External Secrets above.
6. Merge this GitOps application; confirm ArgoCD renders
   `deploy/k8s/overlays/canonical-plus` and the generated names are suffixed
   `-canonical-plus`.
7. Route `/shared-auth/*` to the Canonical Shared Auth Service and all other app
   traffic to the Canonical web/API origins.
8. Verify redirect, login, CSRF rejection, owner isolation, quote submission,
   REST recovery, WebSocket updates, logout/revocation, and refresh rotation.
9. Add Cloudflare DNS/proxy records only after origin health and TLS checks pass.

## Current activation blockers

- The `shared-auth` GitHub organization has a hosted Actions billing/spending
  block, so its server and Worker jobs have not executed. Their PRs must remain
  unmerged until the same gates run on an approved independent runner or hosted
  Actions is restored.
- Secrets and Supabase/RDS identifiers are intentionally not committed.
- The ArgoCD application depends on the Canonical overlay being present on the
  Shared Auth repository's `main` branch.
