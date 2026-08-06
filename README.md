# jello-ws / athleto-app-rs

`jello-ws` is the cluster service identity for the AthletO shop app. The
canonical Rust repository and binary remain `athleto-app-rs` so the existing
deployment and Git history stay stable.

The AthletO shop app serves performance gelatin protein cups. *Wobble hard.
Recover clean.*

A self-contained Rust web app on the "MASH" stack:

- **M**aud — server-rendered HTML (inline dark athletic theme, no asset pipeline)
- **A**xum — HTTP server and routing (plus a websocket endpoint pushing HTML fragments)
- **S**eaORM — entities, pool configuration, transactions, and raw
  `sea_orm::Statement` queries for the locking-heavy hold/checkout paths
- Supabase — GoTrue passwordless provider and authoritative MFA state
- `github.com/shared-auth` — shared session issuance, identity linking, and
  revocation authority
- **H**TMX — add-to-cart / remove-from-cart fragment swaps (vendored, served same-origin)

Products: **Athlet-O Starter** (20g gelatin protein, inulin fiber, vitamin C, electrolytes),
**Recover-O**, and **Pre-Game-O** — each as a ready cup (just add water was already done for you)
and a powder packet (just add water).

## Routes

| Route | What |
| --- | --- |
| `GET /` | Storefront product grid (both formats, prices, calories) |
| `GET /product/{slug}` | Product detail |
| `GET|POST /signup`, `GET|POST /login`, `POST /logout` | Supabase magic-link/MFA provider plus shared-auth sessions; browser-bound provider and shared tokens use HttpOnly Secure SameSite=Lax cookies |
| `GET /cart`, `POST /cart/items`, `POST /cart/items/{id}/delete` | Cart pages + htmx fragments; keyed by Supabase user id or anonymous cart cookie |
| `POST /checkout`, `GET /orders`, `GET /orders/{id}`, `POST /orders/{id}/reorder`, `POST /orders/{id}/pay` | Hosted payment checkout/retry (Stripe, PayPal, Square; approved B2B Net-30 invoices), order history with status/ETA/tracking + B2B filters, printable receipt, reorder |
| `GET /pay/{success,cancel}`, `POST /webhooks/{stripe,paypal,square}` | Verified provider returns and signed, replay-safe payment webhooks |
| `GET|POST /quick-order` | B2B case-quantity grid straight into the cart |
| `GET|POST /api/v1/...` | ERP JSON API (hashed `athk_` keys): products, orders (list/create), `POST /api/v1/orders/{id}/fulfillment` records carrier + tracking (856-style) |
| `POST /api/v1/ops/customers/{user_id}/approval` | Ops-only (operations credential): approve/revoke a business account — the step that lets a vetted B2B customer order + use the ERP API. Body `{"approved": true\|false}`. Until approved, `is_b2b_approved()` gates off B2B ordering, the ERP API, and API keys |
| `GET /ws` | Authenticated websocket pushing HTML fragments (htmx ws extension, `hx-swap-oob`): live cart-hold countdown; `GET /cart/hold` polling remains the fallback |
| `GET /static/...` | Vendored htmx + ws extension, served same-origin with immutable caching |
| `GET /healthz` | Liveness/readiness — always `ok`, no dependencies |

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `8080` | Bind port |
| `SUPABASE_URL` | *(unset)* | Supabase project URL, e.g. `https://xyz.supabase.co` |
| `SUPABASE_ANON_KEY` | *(unset)* | Supabase anon (publishable) key |
| `SHARED_AUTH_BASE_URL` | *(unset)* | Canonical `github.com/shared-auth` authority base. Public URLs must use HTTPS; loopback and Kubernetes service DNS may use HTTP. Required with the two Supabase values for login. |
| `DATABASE_URL` | *(unset)* | Supabase pooled Postgres URL (e.g. the Supavisor `...pooler.supabase.com:6543/postgres` string). TLS is enforced for public hosts automatically — see below. |
| `ATHLETO_DB_SSLMODE` | *(unset)* | Override the auto-selected libpq `sslmode` (`disable`/`require`/`verify-ca`/`verify-full`). Leave unset to get `verify-full` against the pinned Supabase CA on public hosts. |
| `ATHLETO_DB_SSLROOTCERT` | *(bundled Supabase CA)* | Path to a CA file for `verify-full`. Defaults to the embedded `certs/supabase-prod-ca-2021.crt`. Set this when connecting to a non-Supabase public Postgres. |
| `ATHLETO_PUBLIC_BASE_URL` / `ATHLETO_BIZ_PUBLIC_BASE_URL` | `https://app.athleto.store` / `https://biz.athleto.store` | Canonical B2C/B2B browser origins for auth and provider returns |
| `ALLOWED_HOSTS` | *(unset)* | Comma-separated Host-header allowlist (e.g. `app.athleto.store,biz.athleto.store`); unset = permissive with a startup warning |
| `ATHLETO_TRUSTED_PROXY_CIDRS` | *(unset)* | Comma-separated ingress/LB CIDRs allowed to supply `X-Forwarded-For` for abuse throttles. Unset means the app ignores that header and uses the direct peer address. |
| `ATHLETO_ALLOW_SELF_SIGNUP` | `0` | Set to `1` only with both Turnstile values below. Default-deny magic-link requests only sign in existing accounts. |
| `ATHLETO_TURNSTILE_SITE_KEY` / `ATHLETO_TURNSTILE_SECRET` | *(unset)* | Cloudflare Turnstile public site key and private verification key required together for self-signup. The secret is managed through the normal environment/Fiducia envelope path. |
| `ATHLETO_MFA_STATE_KEY` | *(unset)* | Base64-encoded 32-byte HMAC key that signs the five-minute pending SMS-MFA challenge cookie. SMS verification fails closed when absent or malformed. |
| `ATHLETO_OPERATIONS_API_KEY` | *(unset)* | Dedicated bearer credential for ops-only writes: warehouse fulfillment (`POST /api/v1/orders/{id}/fulfillment`) and B2B account approval (`POST /api/v1/ops/customers/{id}/approval`) |
| `ATHLETO_STRIPE_SECRET_KEY` / `ATHLETO_STRIPE_WEBHOOK_SECRET` | *(unset)* | Stripe hosted checkout and signed webhook verification; also enables approved B2B Net-30 invoices |
| `ATHLETO_PAYPAL_CLIENT_ID` / `ATHLETO_PAYPAL_CLIENT_SECRET` / `ATHLETO_PAYPAL_WEBHOOK_ID` | *(unset)* | PayPal hosted checkout/subscriptions and webhook verification |
| `ATHLETO_SQUARE_ACCESS_TOKEN` / `ATHLETO_SQUARE_LOCATION_ID` / `ATHLETO_SQUARE_WEBHOOK_SIGNATURE_KEY` | *(unset)* | Square hosted checkout/subscriptions and signature verification |
| `ATHLETO_BILLING_URL` / `ATHLETO_BILLING_API_KEY` / `ATHLETO_BILLING_TENANT_ID` | *(unset)* | Optional observer-only AR/AP ledger integration |
| `FIDUCIA_URL` / `FIDUCIA_API_KEY` | *(both unset)* | fiducia.cloud control plane for fenced singleton-job leases, distributed login/cart/MFA throttles, and the allowlisted `secrets/athleto/*` KV overlay. Public endpoints require `https`; internal `http` is allowed only for local/cluster addresses. Both unset = environment-only secrets, local-development throttles, and Postgres advisory-lock fallback; partial or unsafe configuration fails closed. |
| `ATHLETO_SECRETS_KEY` | *(unset)* | Legacy migration key for client-encrypted `v1:` KV envelopes only. Prefer Fiducia's external Vault Transit or versioned keyring at-rest protection; node-encrypted and explicitly plaintext KV responses are handled without this key. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | *(unset)* | OTLP/gRPC collector endpoint for traces and metrics; unset keeps structured JSON logs only |
| `OTEL_RESOURCE_ATTRIBUTES` | *(unset)* | Additional non-secret OTEL resource labels; service identity cannot be overridden |

The app starts and serves every page with **no** secrets set: `/healthz` passes, the
storefront renders from a built-in catalog, and auth/cart routes show a
"not configured" notice. Set the two Supabase variables plus
`SHARED_AUTH_BASE_URL` to enable auth, and `DATABASE_URL` for cart persistence.

## Local run

```sh
export SUPABASE_URL=https://<project-ref>.supabase.co
export SUPABASE_ANON_KEY=<anon-key>
export SHARED_AUTH_BASE_URL=http://127.0.0.1:8120/shared-auth
export DATABASE_URL=postgres://postgres.<project-ref>:<password>@<region>.pooler.supabase.com:6543/postgres
cargo run
# then open http://localhost:8080
```

Or just `cargo run` for the degraded (no-secrets) storefront.

Note: session and cart cookies are set with the `Secure` flag; modern browsers
accept them on `http://localhost`.

## Database and migrations

Application database access is SeaORM-only (`src/entities/` + `src/db.rs`).
The hold-claim and checkout transactions remain hand-written SQL executed via
`sea_orm::Statement` to preserve their locking semantics. `DATABASE_URL` is
optional and the SeaORM connection is lazy, so the storefront still boots
degraded when Postgres is unavailable.

The numbered files under `migrations/` are a frozen audit trail. The process
does not run DDL at startup. The current schema authority is the dedicated
`athleto` database contract at
`~/codes/ores/k8s-cluster/remote/libs/pg-defs/schema/databases/athleto/schema.sql`.

### Database connection TLS

`sqlx`'s default `sslmode=prefer` silently falls back to **plaintext** if the
TLS handshake fails and never verifies the server certificate. So on boot,
`db::build_pool` upgrades the connection unless the URL already sets `sslmode`:

- **Local / private / cluster-internal hosts** (CI's `localhost:5432`, dev):
  left as plaintext — that network is the trust boundary.
- **Public hosts** (the Supabase pooler): `sslmode=verify-full` against the
  pinned **Supabase Root 2021 CA** (`certs/supabase-prod-ca-2021.crt`), which
  is a private CA absent from the default rustls root store. If the CA cannot
  be materialized it degrades to `sslmode=require` (encrypted, unverified)
  rather than failing the connection.

Override with `ATHLETO_DB_SSLMODE` / `ATHLETO_DB_SSLROOTCERT` (see the env
table). An explicit `?sslmode=` in `DATABASE_URL` disables all of this.

### Supabase database role and RLS

RLS protects direct PostgREST access today, but it cannot protect a backend
connection made as a table owner or `BYPASSRLS` role. Production must use a
dedicated, non-owner runtime login in `DATABASE_URL`, with `NOBYPASSRLS`, and
policies that constrain every customer-scoped operation to the authenticated
subject. The rollout checklist and validation query are in
[`docs/supabase-rls-runtime.md`](docs/supabase-rls-runtime.md). Do not point
the production app at the Supabase `postgres` owner account once that role is
provisioned.

### Go-forward: declarative migrations (dpm)

[dpm](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)
materializes that contract on a shadow server and emits ordered, reviewable SQL
to converge the target database.

```sh
brew install declarative-migrations/tap/dpm

export SHADOW_DATABASE_URL=postgres://…   # server where dpm may create throwaway DBs
export TARGET_DATABASE_URL=postgres://…   # or DATABASE_URL

scripts/dpm.sh diff      # print migration SQL; never executes
scripts/dpm.sh verify    # rehearse and prove convergence
scripts/dpm.sh review    # diff plus migration review
scripts/dpm.sh apply     # interactive; destructive SQL remains explicitly gated
```

The contract targets its own database named `athleto`, never the shared
`public` schema. Migration application remains a reviewed operator action.

## Observability

The service emits flattened JSON tracing events to stderr for Kubernetes CRI
collection by Promtail/Loki. Every HTTP route has a W3C-parented server span,
bounded route/method/status metrics, and `trace_id`/`span_id` log fields. When
`OTEL_EXPORTER_OTLP_ENDPOINT` is configured, traces and metrics are sent to the
cluster OpenTelemetry Collector; Prometheus scrapes the collector's exporter.
Shared-auth exchange, introspection, and logout calls propagate W3C trace
headers and emit bounded auth outcome spans without recording session tokens.

## Deploy

Deployed at **https://app.athleto.store** from the ORESoftware `k8s-cluster`
repo. The public Ingress routes to the **`jello-ws`** ClusterIP Service, which
selects the existing **`dd-athleto-app-rs`** deployment pods. This repo is
checked out there as a git submodule at
`remote/deployments/athleto-app-rs`; this standalone repository remains the
source of truth, and `k8s-cluster` only bumps the reviewed submodule pointer.
The runtime manifest builds the pinned submodule with `cargo run --release
--locked`, injects database/auth configuration from cluster secrets, injects
pod metadata plus the in-cluster OTLP collector endpoint, and probes
`/healthz`.

## Cross-surface delivery

User-visible, catalog, cart, checkout, order, fulfillment, account,
notification, permission, navigation, or deep-link changes in this Rust web app
must be evaluated for:

- `athlet-o/athleto-flutter` on Android, iOS, Flutter Web/mobile web, and
  Flutter desktop;
- `athlet-o/athleto-desktop.rs`, the planned Rust desktop app for coach,
  facility, team, and analytics workflows; and
- Athlet-O interfaces, generated clients, product/order/payment schemas, route
  types, fixtures, and conformance tests.

This is judgment-based coordination, not blind parity. Public storefront SEO,
server-rendered marketing, and web-only checkout/provider return pages can
remain web-specific. Native coach/facility analytics, local device imports,
notifications, secure storage, and offline workflows may be native-specific.
Catalog/product semantics, cart and order state, account/B2B approval,
payment status, fulfillment, permissions, errors, notifications, and navigation
normally require coordinated changes or an explicit no-change rationale and
parity follow-up.

Deep links are HTTPS-first and should preserve the verified Athlet-O origins:

```text
https://<verified-athlet-o-owned-host>/open/<route>?<bounded-query>
```

A custom-scheme fallback requires a reviewed ADR before registration; do not
invent one. Web, Flutter, and Rust desktop must share versioned route types and
fixtures and support cold start, already-running delivery, authentication
resume, replay/expiry rejection, and browser fallback. Payment credentials,
provider tokens, API keys, cart/session cookies, private order data, health or
fitness data, and personally sensitive information are prohibited in URLs.
Checkout, payment-return, order, approval, and invitation handoffs use bounded
IDs or short-lived, single-use, audience-bound codes and explicit confirmation.

See [`docs/CROSS_SURFACE_DELIVERY.md`](docs/CROSS_SURFACE_DELIVERY.md) and the
[portfolio policy](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md).
