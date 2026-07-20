# billing-server.rs — Quaestor

**[quaestor-ledger](https://github.com/quaestor-ledger)** · site: [quaestor-ledger.github.io](https://quaestor-ledger.github.io)

> Extracted with full history from `ORESoftware/k8s-cluster`
> (`remote/deployments/billing-server-rs`) on 2026-07-17. **This repo is the
> source of truth**; k8s-cluster vendors it back as a submodule at the same path.
> Path deps (`../../libs`, `../../submodules`) resolve inside that superproject
> checkout — full builds happen there. See [AGENTS.md](AGENTS.md) for repo rules.

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
- **Crypto is read-only too.** Tenants connect Solana and Ethereum/EVM wallet
  addresses via wallet signing or explicit metadata. We watch the chain; we
  never request delegated spend authority and never hold private keys.
- **Solana is used for two things:** periodic Merkle-root anchoring of the
  ledger (tamper-evidence) and read-only ingestion of on-chain transfers
  into the same per-entity ledger as fiat. Ethereum/EVM support is observer
  mode only: native balance, ERC-20 balance, and receipt reads through JSON-RPC.

## Source of truth

Postgres. Always. The `postings` table is append-only (UPDATE/DELETE are
forbidden by trigger), and every transaction's postings must sum to zero per
currency (enforced by a deferred constraint trigger).

## Database schema — declarative, via dpm

[`schema/schema.sql`](./schema/schema.sql) is the schema source of truth for
this service's own database (separate from the shared `pg-defs` RDS
contract). The live database converges onto it with
[dpm](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)
through [`scripts/dpm.sh`](./scripts/dpm.sh) — the same workflow as
`remote/libs/pg-defs`:

```sh
export SHADOW_DATABASE_URL=postgres://...   # server where dpm may create throwaway DBs
export TARGET_DATABASE_URL=postgres://...   # or BILLING_DATABASE_URL / DATABASE_URL

scripts/dpm.sh diff        # print the migration SQL (never executes)
scripts/dpm.sh verify      # rehearse on a shadow replica, prove convergence
scripts/dpm.sh review      # diff + AI review
scripts/dpm.sh apply       # generate + execute (interactive confirm)
scripts/dpm.sh bootstrap   # full DDL for an empty database
```

The server **never migrates at boot** (the old `sqlx::migrate!` /
`BILLING_RUN_MIGRATIONS` switch is gone); a human reviews and applies every
schema change. `migrations/` is a frozen historical record — see
[`migrations/README.md`](./migrations/README.md). Data access is SeaORM
(entities in `src/entity/`, one module per table, hand-written against
`schema/schema.sql`).

Customer billing-state snapshots additionally serialize through
[fiducia.cloud](https://fiducia.cloud) when `BILLING_FIDUCIA_ENABLED=true`. The
read path atomically acquires the union of
`billing:customer:<tenant_id>:<customer_id>` keys before rolling up customer
accounts, and `LedgerService::post_transaction` locks the same keys for every
customer account code it mutates (`ar/<id>`, `unallocated_cash/<id>`,
`credit_memo(s)/<id>`, `customer/<id>/...`). The snapshot queries run in one
Postgres `REPEATABLE READ READ ONLY` transaction, so the result cannot stitch
together multiple database snapshots even if a non-cooperating writer exists.
Immediately before a ledger commit or snapshot handoff, the service reacquires
the exact same holder/key set. Fiducia extends the expiry without changing the
fencing token; a missing or different grant aborts the Postgres transaction.

Tenant leases use Fiducia leader elections: campaign = acquire, renew = extend,
and resign = release. The durable billing mirror and audit event commit together
in Postgres under a transaction-scoped advisory lock. Tenant-lease HTTP calls
stay outside database transactions. The customer-lock pre-commit validation is
the deliberate bounded exception: it proves the fencing authority is still
current before Postgres makes a protected change durable. If the Postgres commit
fails after a Fiducia campaign or renewal, the service compensates by resigning
the remote lease.
Customer critical sections keep the independent Postgres advisory lock already
used by ledger idempotency.

Billing consumes the canonical
[`fiducia-cloud/fiducia-clients`](https://github.com/fiducia-cloud/fiducia-clients)
Rust SDK at an exact Git revision; that SDK pins and re-exports the generated
[`fiducia-cloud/fiducia-interfaces`](https://github.com/fiducia-cloud/fiducia-interfaces)
contracts. Both revisions are recorded in `Cargo.lock`, so builds do not silently
float across either boundary. Production requires a least-privilege Fiducia API
key with `locks:write` scope (that scope also authorizes election reads/writes).
Every Fiducia mutation carries an `Idempotency-Key` and uses the SDK's bounded
retry policy; credentials are never logged, and readiness fails closed while
coordination is enabled but unavailable.

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

## Event bus (NATS)

The server publishes **redacted** domain events and listens for inbound sync
commands over NATS, using the shared cross-language subject registry at
`remote/libs/nats/subject-defs` (crate `dd-nats-subject-defs`). It is **off by
default** — set `BILLING_NATS_PUBLISH_ENABLED=true` and a URL
(`BILLING_NATS_URL`, falling back to `NATS_URL`) to turn it on. A broker outage
at boot degrades to a silent no-op rather than blocking the ledger; publishing
is always best-effort and never on a transaction's critical path. See
`src/events.rs`.

Published (`dd.remote.billing.*`):

| subject | when |
|---------|------|
| `…ledger.postings` | a double-entry transaction commits (per-currency totals, no posting detail) |
| `…reconciliation.breaks` | a reconciliation break opens during provider sync |
| `…anchors` | a Merkle root is anchored to Solana |
| `…webhooks.receipts` | a provider webhook is recorded — **hash only, never the body** |
| `…connections.events` | a provider connection is created / attached |

Subscribed: `dd.remote.billing.commands.sync` (queue group `dd-billing-server`)
— a `{tenantId, connectionId}` command is turned into the same one-shot
`sync.connection` job the HTTP "Sync now" path enqueues, so one replica handles
each command and all the lease / rate-limit / dispatch logic is reused.

Envelopes are `{schemaVersion, source, emittedAt, …fields}`; payloads carry no
secrets, raw bodies, or sealed credentials. Publish counters are exposed on
`/metrics` (`dd_billing_server_nats_*`). Tune the size ceiling with
`BILLING_NATS_MAX_PAYLOAD_BYTES` (default 1 MiB) and the inbound queue group
with `BILLING_NATS_QUEUE_GROUP`.

Editing the subject set means editing
`remote/libs/nats/subject-defs/schema/billing.schema.json` and regenerating
(`node src/generate.mjs` in that package); a staleness test guards the
committed outputs.

## Layout

```
src/
  main.rs              # bootstrap + graceful shutdown
  config.rs            # env config
  state.rs             # AppState (services + clients)
  error.rs             # AppError + IntoResponse
  db.rs                # SeaORM connection + raw-Statement helpers
  entity/              # SeaORM entities (one module per table, schema mirror)
  crypto.rs            # per-tenant AES-GCM credential sealing
  fiducia.rs           # async adapter over the revision-pinned Fiducia Rust SDK
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
schema/
  schema.sql           # declarative schema source of truth (dpm converges onto it)
scripts/
  dpm.sh               # diff / verify / review / apply / bootstrap wrapper for dpm
migrations/            # FROZEN historical sqlx migrations — never applied; see its README.md
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

# 2. Create the schema (one-time; the server does NOT migrate at boot).
#    Quick local path — pipe the bootstrap DDL into psql:
export BILLING_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres
export SHADOW_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres
scripts/dpm.sh bootstrap | psql "$BILLING_DATABASE_URL"
#    (against shared/long-lived databases use `scripts/dpm.sh diff` +
#     `scripts/dpm.sh apply` so every change is reviewed first)

# 3. Set env
export BILLING_MASTER_SEAL_KEY="$(openssl rand -base64 32)"
export SOLANA_RPC_URL=https://api.devnet.solana.com
export SOLANA_CLUSTER=devnet
export STRIPE_API_VERSION=2026-04-22.dahlia
# OAuth app secret, used only for Stripe Connect code exchange.
export STRIPE_CLIENT_SECRET=...
# Stripe platform API key, used for Stripe API reads with Stripe-Account.
export STRIPE_API_KEY=...
# Provider webhook secrets are optional locally; strict mode is now the default.
export STRIPE_WEBHOOK_SECRET=whsec_...
# Webhook signature verification defaults to ON (fail-closed). Only turn it off
# for local development against unsigned mock payloads.
export BILLING_REQUIRE_WEBHOOK_SIGNATURES=false
# Fail-closed auth. The server refuses to boot when any of these hold:
#   - the admin UI is enabled and BILLING_ADMIN_AUTH_BEARER is unset
#   - BILLING_API_AUTH_BEARER is unset
#   - tenant routes require a user JWT (the default) but Supabase is unconfigured
# For local dev, either set all of them or opt out explicitly:
export BILLING_ALLOW_INSECURE_DEV=1

# --- Per-user Supabase auth (see "Auth posture" below) ---
# Enables per-user JWT verification. Issuer and JWKS URL are derived from this.
export BILLING_SUPABASE_URL=https://<project-ref>.supabase.co
# Optional; defaults shown.
# export BILLING_SUPABASE_JWT_AUD=authenticated
# Override these two only for a self-hosted GoTrue that doesn't follow the
# hosted URL layout:
# export BILLING_SUPABASE_JWT_ISS=https://<project-ref>.supabase.co/auth/v1
# export BILLING_SUPABASE_JWKS_URL=https://<project-ref>.supabase.co/auth/v1/.well-known/jwks.json
# LEGACY symmetric secret. Leave unset on any project using JWKS signing keys —
# setting it widens the accepted algorithm set to include HS256 for no benefit.
# export BILLING_SUPABASE_JWT_SECRET=...
# Defaults to true (fail-closed). See the migration path in "Auth posture".
# export BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false
export BILLING_FIDUCIA_ENABLED=false # set true with a local/public Fiducia endpoint
# export BILLING_FIDUCIA_BASE_URL=http://127.0.0.1:8088
# export BILLING_FIDUCIA_API_KEY=fdc_... # requires locks:write
export RUST_LOG=info,sqlx=warn

# 4. Run
cargo run --release
```

The server listens on `:8087` by default. It never runs migrations at boot;
schema changes go through `scripts/dpm.sh` (see "Database schema —
declarative, via dpm" above). Production schema changes are always
operator-reviewed; application startup never applies DDL.

## Provider API tests

Provider polling/OAuth clients should be tested against the in-process mock
server in `src/providers/mock_http.rs`, not by calling live provider sandboxes.
The mock asserts method, path, query, headers, and JSON bodies while returning
provider-shaped JSON that deserializes through the same Rust DTOs used in
production. Dedicated API structs expose `with_base_url_for_tests(...)`; inline
Config-driven clients use the `BILLING_*_API_BASE` test/operator overrides.

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
- `remitly`: optional limited-fit partner export fields
  `{ "api_key", "partner_id", "api_base_url", "watched_recipients", "environment", "notes" }`
- `moneygram`: `{ "client_id", "client_secret", "agent_partner_id", "user_language", "environment", "webhook_secret" }`
- `western_union`: `{ "client_id", "environment", "client_certificate_pem", "client_private_key_pem", "notes" }`
- `us_bank_zelle`: `{ "access_token", "client_id", "program_id", "api_base_url", "payments_path", "enrollment_path", "environment" }`
- `jpmorgan_zelle`: `{ "access_token", "debtor_account_id", "debtor_name", "debtor_bic", "api_base_url", "environment" }`
- `bofa_cashpro_gdd`: `{ "client_id", "client_secret", "cashpro_company_id", "access_token", "api_base_url", "disbursements_path", "environment" }`
- `modern_treasury`: `{ "organization_id", "api_key", "default_originating_account_id", "api_base_url", "environment", "webhook_secret" }`
- `dwolla`: `{ "access_token", "account_id", "api_base_url", "environment", "webhook_secret" }`
- `ethereum_wallet`: `{ "address", "rpc_url", "chain_id", "rpc_bearer_token", "tracked_assets" }`
- `adyen`: `{ "api_key", "merchant_account", "environment", "api_base_url", "hmac_key_hex" }`
- `square`: `{ "access_token", "environment", "merchant_id", "webhook_signature_key", "webhook_notification_url" }`

`environment` is `production` or `sandbox`. For Coinflow, Wise, Remitly,
MoneyGram, Western Union, bank-sponsored Zelle providers, Modern Treasury, and
Dwolla the server derives `external_account_id` from the credential payload when
the caller does not provide it. Remitly, MoneyGram, Western Union, Zelle,
Modern Treasury, Dwolla, and Ethereum wallet support are accepted as
`limited_fit`: typed provider DTOs and mock tests exist, but automatic ledger
sync and public money movement are intentionally disabled until a tenant's
contract maps cleanly to postings.

Remitly partner-export credentials are all-or-nothing: `api_key` and
`api_base_url` must be provided together, and the base URL must be an HTTPS
public provider hostname with no URL credentials, query, or fragment. Western
Union mTLS certificate/key PEMs are accepted only as a pair and validated before
the credential payload is sealed.

Bank-sponsored Zelle, Modern Treasury, Dwolla, and tenant-supplied Ethereum RPC
base URLs must be HTTPS public provider hostnames with no URL credentials,
query, or fragment. Localhost/private addresses are accepted only through
test-only mock constructors.

## Webhook posture

Inbound webhook payloads are stored with `signature_ok`, `payload_sha256`,
`verification_error`, `tenant_id`, `connection_id`, and the provider external
account id when it can be inferred. `BILLING_REQUIRE_WEBHOOK_SIGNATURES`
defaults to `true` (fail-closed); unsigned or unverifiable deliveries are
recorded and then rejected with `401`. Set it to `false` only for local
development against unsigned mock payloads.

When a delivery carries no extractable provider event id, the idempotency key
is derived deterministically from the body's `payload_sha256` (not a random
uuid), so repeated deliveries of the same body dedup via the
`(provider, external_event_id)` upsert instead of inserting a new row each time.

**Strict mode also rejects** any signed delivery that cannot be bound to a
tenant connection. That stops a valid platform-secret signature (Stripe
Connect, Plaid, etc.) from being accepted with `tenant_id = NULL` and
silently routed nowhere.

The public ack returns `{"received": true}` only — `tenant_id`,
`connection_id`, and the synthesized event id are deliberately NOT echoed
so that probing senders can't enumerate valid identifiers.

Implemented verification:

- Stripe `Stripe-Signature` HMAC with timestamp replay tolerance.
- PayPal `verify-webhook-signature` API using `PAYPAL_WEBHOOK_ID`.
- Coinbase Commerce HMAC via `x-cc-webhook-signature`.
- Coinflow HMAC via `x-coinflow-signature`.
- Plaid `plaid-verification` ES256 JWT with `request_body_sha256` claim,
  via cached JWKS lookups.
- Bridge.xyz RSA-SHA256 PKCS1v15 with timestamp freshness, key sourced
  from the per-connection sealed credential.
- Fireblocks RSA-SHA512 PKCS1v15, key sourced from the per-connection
  sealed credential.
- Revolut, GoCardless, Mercury, Circle: HMAC-SHA256 with per-connection
  secret (falls back to env secret only in non-strict mode).
- Modern Treasury (`X-Signature`) and Dwolla (`X-Request-Signature-SHA-256`):
  HMAC-SHA256 (hex) over the raw body with the per-connection webhook secret.
  Dwolla deliveries bind to their connection via the account/customer id in
  `_links`. Modern Treasury event bodies carry no stable routing key, so in
  strict mode MT webhooks cannot bind to a connection and are recorded but
  rejected (a dedicated per-connection webhook path/secret-id is the
  follow-up); the verifier is reachable once a routing key is wired.
- Square `x-square-hmacsha256-signature`: HMAC-SHA256 over
  `notification_url + body` (base64), keyed by the per-connection signature
  key — the registered notification URL is stored alongside the credential
  because it participates in the signature.
- Adyen: HMAC-SHA256 (base64) over the `:`-joined NotificationRequestItem
  field string, keyed by the merchant HMAC key (hex). The signature travels
  inside the payload (`additionalData.hmacSignature`), not a header.

**Payloads are encrypted at rest.** Inbound bodies are sealed with the master
AES-256-GCM key (`src/crypto.rs`) into `webhook_events.payload_sealed`; the
plaintext `payload` column is no longer written. The clear-text
`payload_sha256` is retained for dedup/correlation.

## Auth posture (2026-05-23 hardening)

The JSON API is gated by a single in-process bearer token —
`BILLING_API_AUTH_BEARER` — in addition to whatever upstream gateway
(`dd-remote-auth`, ALB OIDC, …) is in front of the listener. The bearer
is a fail-closed floor for any reachable-from-network deployment.

```
Authorization: Bearer <BILLING_API_AUTH_BEARER>
```

Exempted paths (no bearer required):

- `/healthz`, `/readyz`, `/metrics` — orchestrator probes
- `/v1/webhooks/*` — provider signatures are the auth model
- `/v1/verify/*` — public anchor verification by design
- `/v1/oauth/*/callback` — the single-use `state` token is the CSRF guard
- `/admin/*` — `BILLING_ADMIN_AUTH_BEARER` governs this nest separately

The OAuth `/start` and Plaid `/link-token`/`/exchange` endpoints **do**
require the bearer — they mint per-tenant CSRF state and seal
credentials, so they have to prove the caller's identity.

The bearer is now **required to boot** (fail-closed): with
`BILLING_API_AUTH_BEARER` unset the server refuses to start rather than run
the API in open mode, unless `BILLING_ALLOW_INSECURE_DEV=1` is set explicitly
for local development (in which case it runs open and logs a single WARN line
at boot). Production manifests inject the bearer via SealedSecrets /
ExternalSecrets.

### Other 2026-05-23 hardening fixes

- **Scheduler routes are tenant-scoped.** `get_one`, `list_runs`,
  `run_now`, `enable`, `disable` all UPDATE/SELECT with both `id` AND
  `tenant_id`. Cross-tenant access returns `404 Not Found` so a leaked
  UUID can't be probed.
- **Connection UPDATEs always carry `AND tenant_id = $X`** (defense in
  depth; helps when a future caller learns a connection UUID through a
  side channel).
- **Webhook routing is unique-active per `(provider, external_account_id)`**
  via a partial unique index. Misconfigurations now fail at INSERT time
  rather than producing ambiguous "most-recently-updated wins" routing.
- **Ledger `POST /transactions` rejects** when `body.tenant_id` is set
  and disagrees with the path `tenant_id`. Nil bodies are still accepted
  (the handler fills in the path value).
- **Idempotency races are closed** via
  `pg_advisory_xact_lock(tenant_part, hash(idempotency_key))` so two
  concurrent calls with the same key always see the same (committed)
  result.
- **Customer snapshots are transactionally consistent.** Fiducia excludes
  cooperating cross-service writers while a Postgres repeatable-read, read-only
  transaction gives all component queries one MVCC snapshot.
- **Tenant lease mirrors and audit events are atomic.** Acquire, renew, release,
  and expiry sweep mutations use Postgres transactions plus transaction-scoped
  advisory locks, with bounded compensation for Fiducia/Postgres split outcomes.
- **Scheduler outcomes are atomic.** A run status, dead-letter insertion, and
  next schedule timestamp commit together instead of leaving partial state.
- **Notification throttling is atomic.** A per-rule advisory lock protects the
  daily count-and-insert transaction so concurrent evaluators cannot exceed the
  configured limit.
- **OAuth `return_to` requires an explicit allowlist.** The previous
  "any path starting with `/`" auto-permit is gone; protocol-relative
  values (`//evil.example/...`) are also rejected.
- **Outbound HTTP is SSRF-guarded.** Notification webhooks and the
  `tenant.webhook` scheduled job refuse literal private / loopback /
  link-local / CGNAT / metadata IPs and any non-http(s) scheme. DNS
  rebinding is left to the network policy; this is the literal-IP
  defense at the application layer. Toggle via
  `BILLING_BLOCK_PRIVATE_OUTBOUND` (default `true`).
- **Notification rule `credential_plaintext_b64` is now rejected** with
  400 (was silently dropped). Will be re-opened once the per-rule
  sealing path lands.

### Known follow-ups (not fixed in this pass)

- Per-tenant envelope encryption (currently a single
  `BILLING_MASTER_SEAL_KEY`).
- Solana memo encode/decode for round-tripping anchored Merkle roots
  (currently `onchain_root_matches` returns true when a transaction
  exists at the slot without comparing roots).
- Scheduler exactly-once via `(job_id, scheduled_for)` dedup index; the
  runner is at-least-once today.

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

- **Bearer auth (required when the UI is enabled)** — set
  `BILLING_ADMIN_AUTH_BEARER=<token>` to require `Authorization: Bearer
  <token>` on every `/admin/*` request. Constant-time compared. The server
  now refuses to boot with the admin UI enabled and this bearer unset (an
  unauthenticated admin surface), unless `BILLING_ALLOW_INSECURE_DEV=1` is set
  explicitly for trusted networks / local dev.
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

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
