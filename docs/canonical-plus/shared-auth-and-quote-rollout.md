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
| Browser application | `canonical-cloud/canonical-web-server.rs` | Maud/HTMX pages, origin-side Shared Auth verification, owner-scoped REST/WebSockets, SeaORM persistence, and the in-process quote analysis path |
| Durable quote API | `canonical-cloud/canonical-api-server.rs` | Standalone Axum REST/WebSocket service, durable PostgreSQL work claiming, model calls, retries, and strict response validation |
| Browser identity authority | `shared-auth/shared-auth-server.rs` | Magic-link/OTP ceremony, host-only cookies, JWT verification, refresh, and revocation |
| Edge implementation | `shared-auth/shared-auth-infra` | Generic protected-prefix routing, token verification, caller-header stripping, and sanitized identity forwarding |
| Canonical edge deployment | `canonical-cloud/canonical-infra` | Byte-verified Worker source/test mirror, Canonical routes/realm variables, render-only workload seed, and fail-closed activation policy |
| GitOps | `ORESoftware/k8s-cluster` | ArgoCD project/application and tenant boundary |
| Product data plane | Canonical Supabase + PostgreSQL | Independent Canonical identity, session, context, and quote records |

## Landed source state

The reviewed application changes are merged:

- `shared-auth/shared-auth-server.rs#41` — merge commit
  `22aab1de937620251f4e0b9a617c485733c97ff5`;
- `shared-auth/shared-auth-infra#9` — merge commit
  `6234f1ee72349f84652c85a5a957b2982ea471bf`;
- `canonical-cloud/canonical-web-server.rs#41` — squash commit
  `74448c8dcb885fbb240ac59d1079a929bd06caa5`;
- `canonical-cloud/canonical-api-server.rs#6` — squash commit
  `91ac093bf6c3d0958918fc8678af95dd13975f1e`;
- `canonical-cloud/canonical-api-server.rs#8` — squash commit
  `e3a7cc79b3ceac0e455b9d7822a29d4154c9584b`;
- `canonical-cloud/canonical-infra#4` — squash commit
  `03d37469a6ea5ee075a89c064ee60017ae4ebf23`.

The web and standalone API services now agree on:

- default model identifier: `gemini-3.6-pro`;
- explicit runtime override: `GEMINI_MODEL`;
- PostgreSQL context key: `quote-analysis`.

Canonical infra carries byte-for-byte copies of the reviewed Worker source and
Node test blobs from Shared Auth infra commit
`6234f1ee72349f84652c85a5a957b2982ea471bf`; provenance and Git blob hashes are
validated as a single contract. This avoids a second behavioral Worker fork
while also avoiding an unavailable cross-organization private-submodule token.

Competing branches that used `gemini-3.1-pro-preview`, removed durable lease
recovery, reintroduced the divergent local Worker, or claimed overlapping
Kubernetes/Argo objects were closed as superseded.

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

- issuer: `https://app.canonical.plus/shared-auth`;
- audience: `canonical-plus-web`;
- cookie: `__Host-canonical-customer-auth`;
- independent Supabase project;
- independent PostgreSQL identity/session plane and credentials;
- independent signing key;
- independent browser return-state sealing key;
- independent Redis namespace or endpoint.

Do not share these values with `fiducia.cloud`, `oresoftware.com`, or the generic
Shared Auth deployment. Cross-product SSO, when desired, must be an explicit
federation or grant protocol rather than shared cookies or database rows.

## Quote analysis boundary

Both quote implementations combine:

1. version-controlled Markdown policy;
2. the active PostgreSQL `canonical_context` record with key
   `quote-analysis`; and
3. the authenticated customer's validated intake.

Gemini receives bounded context and a structured response schema. Customer and
context fields are untrusted data, not instructions. The result is a preliminary
scope, timeline, and investment range for human review, never an audit opinion,
certification, attestation, or legal conclusion.

The operator-selected source default is `gemini-3.6-pro`; `GEMINI_MODEL` remains
an explicit runtime override. As of August 6, 2026, public Google Gemini API
documentation lists `gemini-3.6-flash` and `gemini-3.1-pro-preview`, but does not
list a public `gemini-3.6-pro` endpoint. Production activation is therefore
fail-closed until the authenticated model inventory for the selected Canonical
Google project and region returns the exact `gemini-3.6-pro` identifier. Do not
silently substitute Flash or another Pro version.

PostgreSQL remains authoritative. REST reads recover current status; WebSocket
messages are notification hints. The asynchronous states are `queued`,
`analyzing`, `ready`, and `failed`.

## Required secrets and identities

The Canonical Shared Auth overlay reads only these External Secrets paths:

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

The quote services additionally require Canonical-only runtime values for:

- PostgreSQL runtime connectivity;
- a separately scoped privileged migration identity used only by a migration
  job;
- Canonical Supabase URL and publishable key;
- a random web-to-API service credential;
- `GEMINI_API_KEY`;
- optional `GEMINI_MODEL`, defaulting to `gemini-3.6-pro`;
- `QUOTE_CONTEXT_KEY=quote-analysis`.

The long-lived web and API processes must never receive the privileged migration
credential. Secrets, account identifiers, provider credentials, and private
endpoints are intentionally excluded from Git.

## Cloudflare reference contract

`canonical-cloud/canonical-infra` contains the Canonical deployment contract:

- Worker name `canonical-plus-auth-edge`;
- zone name `canonical.plus`;
- protected route patterns for `app.canonical.plus/u/*`, the quote REST paths,
  and quote WebSockets;
- same-origin Shared Auth issuer and login paths;
- host-only access and refresh cookie namespaces;
- byte-verified copies of the exact reviewed Shared Auth Worker source and test
  blobs;
- no committed account identifier, token, DNS target, or R2 binding.

That repository is a source contract, not evidence that the Worker, routes, DNS
records, or production environment currently exist in the authorized Cloudflare
account. Before any Cloudflare write, an authenticated inventory must prove:

1. the token resolves to the authorized Canonical account;
2. the exact zone named `canonical.plus` belongs to that account;
3. the exact Worker script and environment;
4. the existing routes and their zone association;
5. the exact `app.canonical.plus` and `api.canonical.plus` DNS records;
6. the exact Kubernetes gateway, load balancer, or tunnel origin;
7. origin health and TLS before enabling proxying.

The current execution runtime has neither DNS nor direct-IP egress to the
Cloudflare API. The supplied token was therefore not sent, and no authenticated
Cloudflare state was read or changed.

No R2 bucket is part of the quote architecture. Do not create, modify, or bind an
R2 bucket unless a separate Canonical use case and exact bucket are approved.

## Ordered activation

1. Apply the desired Canonical PostgreSQL schema with the privileged migration
   identity, then remove that identity from every long-lived runtime.
2. Seed exactly one active `canonical_context` version for `quote-analysis` and
   verify both services read the same row.
3. Provision the exact Canonical-only Shared Auth and quote secrets. Verify that
   no unrelated product value is referenced.
4. Query the selected Google project's authenticated model inventory and require
   the exact `gemini-3.6-pro` identifier before enabling analysis traffic.
5. Build and publish immutable image digests for Shared Auth, the web service,
   the standalone API, and the session revoker; record the rollback digests.
6. Render this ArgoCD Application against the intended cluster and namespace.
   Merge only after the secret objects and image digests are proven. Automated
   sync remains disabled; activation requires an explicit operator sync.
7. Perform the authenticated Cloudflare account, zone, Worker, route, DNS, and
   origin inventory described above. Stop on any ambiguity.
8. Configure `/shared-auth/*` to the Canonical Shared Auth Service and route the
   remaining application and API paths only to the proven Canonical origins.
9. Verify origin health and TLS, then create or update only the explicitly
   approved proxied Canonical DNS records and Worker routes.
10. Certify redirect, magic-link/OTP login, sealed return, CSRF rejection,
    revocation, refresh rotation, owner isolation, quote submission, model
    failure behavior, REST recovery, and WebSocket updates in the deployed
    environment.

## Current activation state

Completed:

- public CTA and exact signed-in destination;
- Shared Auth browser-session implementation;
- reviewed generic edge Worker implementation and 12/12 Node 22 contract tests;
- Canonical edge deployment contract with byte-verified Worker provenance,
  exact routes, realm variables, cookie namespaces, and no credentials;
- signed-in web quote application and embedded API;
- standalone durable quote API;
- shared model and `quote-analysis` context-key contracts;
- PostgreSQL and CockroachDB RLS tests, declarative schema convergence, browser
  E2E, RustSec active-graph validation, and non-root image contracts;
- manual-only ArgoCD application policy;
- Linear activation tracking;
- closure of conflicting wrong-model and duplicate-Worker branches.

Remaining fail-closed gates:

- privileged schema application and `quote-analysis` seed record;
- Canonical-only Supabase, PostgreSQL, Shared Auth, service-token, and Gemini
  secrets;
- authenticated proof that `gemini-3.6-pro` exists for the selected project and
  region;
- immutable production image digests and rollback references;
- authenticated Cloudflare account and exact `canonical.plus` zone ownership;
- exact Worker, route, DNS, environment, and Kubernetes origin inventory;
- origin health and TLS;
- manual ArgoCD sync and deployed end-to-end certification.

No Cloudflare, DNS, R2, Supabase, PostgreSQL, secret-store, or live Kubernetes
mutation is authorized by this document alone. Every activation write remains
conditional on the exact target being proven immediately beforehand.
