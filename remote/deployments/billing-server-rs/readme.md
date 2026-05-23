# billing-server-rs

Multi-tenant AR/AP ledger server. HTTP/JSON, Rust, axum, Postgres source of
truth, Solana as a tamper-evidence notary.

This service answers the two questions from
[`docs/billing-platform-brief.md`](../../../docs/billing-platform-brief.md):

1. **When do I bill the customer, and for how much?** →
   `GET /v1/tenants/{tenant_id}/customers/by-email/{email}/billing-state`
2. **When do I pay a vendor, and how much?** →
   `GET /v1/tenants/{tenant_id}/vendors/by-email/{email}/payable-state`

## Posture

- **Model A** (observer / recorder). We never move money on our own license.
  Tenants connect their own Stripe / PayPal / Braintree / Plaid / bank
  accounts via OAuth (where supported) or sealed API keys. We read, ledger,
  and reconcile; tenants initiate payments in their own provider dashboards.
- **Crypto is read-only too.** Tenants connect a Solana wallet pubkey via
  wallet-adapter signing. We watch the chain; we never request delegated
  spend authority and never hold private keys.
- **Solana is used for two things:** periodic Merkle-root anchoring of the
  ledger (tamper-evidence) and read-only ingestion of on-chain transfers
  into the same per-entity ledger as fiat.

## Source of truth

Postgres. Always. The `postings` table is append-only (UPDATE/DELETE are
forbidden by trigger), and every transaction's postings must sum to zero per
currency (enforced by a deferred constraint trigger).

The `anchors` table records Merkle roots committed to Solana so any third
party can independently verify a posting was present at a given on-chain
slot via `GET /v1/verify/tenants/{tenant_id}/postings/{id}`.

## Sharding

Every tenant-scoped row carries `shard_key BIGINT` derived from
`(tenant_id, region)`. Region is a regulatory boundary
(`US:{state}` / `EU:{country}` / `OTHER:{country}`), not just a hash bucket,
because data-residency requirements often demand a tenant's ledger never
leave a given jurisdiction. The first physical shard is single-region; the
sharding abstraction is in place from day 1 so adding additional database
clusters per region requires no schema change.

## Layout

```
src/
  main.rs              # bootstrap + graceful shutdown
  config.rs            # env config
  state.rs             # AppState (services + clients)
  error.rs             # AppError + IntoResponse
  db.rs                # PG pool + migrations
  crypto.rs            # per-tenant AES-GCM credential sealing
  money.rs             # Money / Currency (minor units, integer)
  shard.rs             # ShardKey + Region
  ledger/              # double-entry posting + balance + invariants
  tenants.rs           # tenant CRUD
  users.rs             # per-tenant customer/vendor entities (uniq by email)
  customers.rs         # Q1 — billing-state aggregation
  vendors.rs           # Q2 — payable-state aggregation + rail selection
  providers/           # OAuth/API-key/wallet connection model + stubs
    stripe.rs paypal.rs braintree.rs coinbase.rs
    plaid.rs swift.rs solana.rs wise.rs
    connection.rs      # sealed-credential storage
  solana/              # anchor service + RPC client + merkle + verify
  api/                 # axum router + handlers
migrations/
  20260518000001_init.sql            # tenants, users, accounts, transactions, postings
  20260518000002_connections.sql     # provider connections, OAuth state, webhook events
  20260518000003_reconciliation.sql  # breaks, anchors
k8s/ec2/
  dd-billing-server.deployment.yaml
  dd-billing-server.service.yaml
  dd-billing-server-secrets.externalsecret.yaml
  kustomization.yaml
Dockerfile             # multi-stage; for future containerized deploy
```

The Argo CD Application is registered at
`remote/argocd/apps/dd-billing-server.application.yaml` and tracks
`dev` branch.

## Running locally

```bash
# 1. Bring up Postgres (any 14+ works)
docker run --rm -d --name billing-pg \
  -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16

# 2. Set env
export BILLING_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres
export BILLING_MASTER_SEAL_KEY="$(openssl rand -base64 32)"
export SOLANA_RPC_URL=https://api.devnet.solana.com
export SOLANA_CLUSTER=devnet
export STRIPE_API_VERSION=2026-04-22.dahlia
# OAuth app secret, used only for Stripe Connect code exchange.
export STRIPE_CLIENT_SECRET=...
# Stripe platform API key, used for Stripe API reads with Stripe-Account.
export STRIPE_API_KEY=...
# Provider webhook secrets are optional locally; set strict mode in shared envs.
export STRIPE_WEBHOOK_SECRET=whsec_...
export BILLING_REQUIRE_WEBHOOK_SIGNATURES=false
export RUST_LOG=info,sqlx=warn

# 3. Run
cargo run --release
```

The server listens on `:8087` by default. Migrations run automatically on
boot unless `BILLING_RUN_MIGRATIONS=false`.

## End-to-end smoke flow

```bash
BASE=http://localhost:8087

# 1. Create a tenant
TENANT=$(curl -s -X POST $BASE/v1/tenants \
  -H 'content-type: application/json' \
  -d '{"slug":"dancingdragons","display_name":"Dancing Dragons",
       "country_code":"US","us_state":"CA"}' | jq -r .id)

# 2. Create a customer (will be billed)
curl -s -X POST $BASE/v1/tenants/$TENANT/users \
  -H 'content-type: application/json' \
  -d '{"email":"alice@example.com","display_name":"Alice","is_customer":true}'

# 3. Create the per-customer AR account
USER_ID=$(curl -s $BASE/v1/tenants/$TENANT/users/by-email/alice%40example.com | jq -r .id)
curl -s -X POST $BASE/v1/tenants/$TENANT/accounts \
  -H 'content-type: application/json' \
  -d "{\"kind\":\"receivable\",\"code\":\"ar/$USER_ID\",\"currency\":\"USD\",
       \"user_id\":\"$USER_ID\"}"
curl -s -X POST $BASE/v1/tenants/$TENANT/accounts \
  -H 'content-type: application/json' \
  -d '{"kind":"income","code":"revenue/saas","currency":"USD"}'

# 4. Bill the customer $19.99
curl -s -X POST $BASE/v1/tenants/$TENANT/transactions \
  -H 'content-type: application/json' \
  -d "{
    \"tenant_id\":\"$TENANT\",
    \"kind\":\"invoice_issued\",
    \"idempotency_key\":\"inv_2026_05_001\",
    \"description\":\"May 2026 subscription\",
    \"postings\":[
      {\"account_code\":\"ar/$USER_ID\",\"direction\":\"debit\",
       \"amount_minor\":1999,\"currency\":\"USD\",
       \"source\":\"manual\",\"source_event_id\":\"inv_2026_05_001/ar\"},
      {\"account_code\":\"revenue/saas\",\"direction\":\"credit\",
       \"amount_minor\":1999,\"currency\":\"USD\",
       \"source\":\"manual\",\"source_event_id\":\"inv_2026_05_001/rev\"}
    ]
  }"

# 5. Read Q1: billing-state
curl -s "$BASE/v1/tenants/$TENANT/customers/by-email/alice%40example.com/billing-state"
```

## Provider connection payloads

`POST /v1/tenants/{tenant_id}/connections/{connection_id}/attach-api-key`
validates the known API-key providers before sealing credentials:

- `coinflow`: `{ "api_key", "merchant_id", "environment", "webhook_validation_key" }`
- `coinbase_commerce` / `coinbase_prime`: `{ "api_key", "webhook_secret", "variant" }`
- `wise`: `{ "api_token", "profile_id", "environment" }`

`environment` is `production` or `sandbox`. For Coinflow and Wise the server
derives `external_account_id` from the credential payload when the caller does
not provide it.

## Webhook posture

Inbound webhook payloads are stored with `signature_ok`, `payload_sha256`,
`verification_error`, `tenant_id`, `connection_id`, and the provider external
account id when it can be inferred. Set
`BILLING_REQUIRE_WEBHOOK_SIGNATURES=true` outside local development; unsigned
or unverifiable deliveries are recorded and then rejected with `401`.

Implemented verification:

- Stripe `Stripe-Signature` HMAC with timestamp replay tolerance.
- PayPal `verify-webhook-signature` API using `PAYPAL_WEBHOOK_ID`.
- Coinbase Commerce HMAC via `x-cc-webhook-signature`.
- Coinflow HMAC via `x-coinflow-signature`.

Plaid webhook JWT verification is not enabled yet; Plaid events are recorded
as unverified, and strict mode rejects them. Backstop/on-demand
`/transactions/sync` remains the safe path until the ES256/JWK verifier lands.

## Admin UI

The server ships with a read-mostly HTMX admin surface at `/admin` (the
JSON API is untouched). It uses [Maud](https://maud.lambda.xyz/) for
compile-time HTML templates plus [HTMX](https://htmx.org/) 2.0
**vendored into the binary** and served from `/admin/static/htmx-<hash>.js`
with SRI integrity — no client toolchain, no bundler, no extra
container, no CDN fetched at runtime.

What you get:

- `/admin/` — dashboard with tenant / connection / job counts, a 5-second
  auto-refreshed status pill, and the most recent job runs across all
  tenants. All four counts are fetched in parallel so dashboard latency
  is bounded by the slowest query, not their sum.
- `/admin/tenants` — list table with an inline HTMX create form that
  prepends new rows without a full reload. The form's inputs carry
  `pattern` / `minlength` / `maxlength` attributes that mirror the
  server-side validators in `admin/validation.rs`.
- `/admin/tenants/{id}` — tenant detail with HTMX-swapped tabs for
  Connections, Scheduled jobs, Leases, and Notifications. URLs are
  pushed (`hx-push-url`) so the active tab survives reloads and shares.
- Inline HTMX actions: `Run now` and `Enable/Disable` on scheduled jobs,
  `Sync now` on provider connections. Each returns just the updated row,
  is gated by an `hx-confirm` prompt, is tenant-scoped at the URL level
  (`/admin/tenants/{tid}/jobs/{id}/run-now`), and is verified for
  ownership before any side effect. Every write emits a structured
  `admin.action=…` audit log line.

### Security posture

Layered defenses, designed to fail safely (see `src/admin/security.rs`
and the wire-level tests in `src/admin/mod.rs`):

- **Bearer auth (optional)** — set `BILLING_ADMIN_AUTH_BEARER=<token>`
  to require `Authorization: Bearer <token>` on every `/admin/*`
  request. Constant-time compared. When unset, the UI is unauthenticated
  (intended for trusted networks / local dev).
- **CSRF guard** — every POST/PUT/PATCH/DELETE must carry
  `HX-Request: true` (HTMX always sends it; cross-origin browsers
  cannot set it without a CORS preflight we do not grant) **and**, when
  `Origin` is present, must come from the request `Host` or an entry in
  `BILLING_ADMIN_ALLOWED_ORIGINS`.
- **Strict CSP** — `default-src 'self'`, `script-src 'self'`,
  `frame-ancestors 'none'`, `object-src 'none'`. No `'unsafe-eval'`, no
  inline scripts, no third-party origins.
- **Security headers on every response** — `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`,
  `Cross-Origin-{Opener,Resource}-Policy: same-origin`, a restrictive
  `Permissions-Policy`, and `X-Robots-Tag: noindex, nofollow, noarchive`.
- **Sanitized errors** — handler failures are logged in full via
  `tracing::warn!` but rendered to the user as `<action>: <kind> — check
  server logs for details`. PG error text, schema names, and stack
  fragments do not leak into HTML.
- **Asset integrity verified at startup** — `assets::verify_integrity()`
  recomputes the SHA-384 of the embedded htmx bytes and panics if they
  drift from the pinned constant, so a sloppy vendor bump cannot ship
  unverified JS to browsers.

### Disabling / fronting

Disable in production environments that have not yet wired
`dd-remote-auth` in front by setting `BILLING_ADMIN_UI_ENABLED=false`.
Per `AGENTS.md`, public gateway paths must stay authenticated. With
`BILLING_ADMIN_AUTH_BEARER` set, the admin UI is safe to leave
mounted behind a TLS-terminating gateway even when `dd-remote-auth` is
the SSO layer in front.

## What is intentionally stubbed in this scaffold

- Provider OAuth code-exchange bodies (Stripe / PayPal / Braintree / Plaid)
  — surface and storage are real; Stripe, PayPal, Braintree, and Plaid token
  exchanges are wired, while each provider still needs broader end-to-end
  sandbox coverage.
- Plaid webhook JWT verification — this needs a vetted ES256/JWK library and
  cache. The ingestor must not act on unverified events.
- Solana memo submission — the anchor service computes the Merkle root and
  persists the `anchors` row, but the on-chain transaction body and signing
  loop is the next piece of work. Verification falls back to "not yet
  anchored" until that lands.
- Plaid `/transactions/sync` posting loop — connection storage is real;
  the worker contract is present, but the exact transaction normalization is
  still pending.
- Wise balance-statement parser — the current Wise sync scans profile
  activities and opens reconciliation breaks for unposted activity; exact
  postings should come from Wise balance statements, not display strings.

These are all deliberately structured as "fill in the function body" tasks
rather than "rearchitect the module" — the boundaries and types are stable.
