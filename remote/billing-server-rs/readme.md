# billing-server-rs

Multi-tenant AR/AP ledger server. HTTP/JSON, Rust, axum, Postgres source of
truth, Solana as a tamper-evidence notary.

This service answers the two questions from
[`docs/billing-platform-brief.md`](../../docs/billing-platform-brief.md):

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
    plaid.rs swift.rs solana.rs
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

## What is intentionally stubbed in this scaffold

- Provider OAuth code-exchange bodies (Stripe / PayPal / Braintree / Plaid)
  — surface and storage are real; the actual HTTP POST to each provider's
  `/oauth/token` endpoint returns `Provider::stub` until you wire it.
- Webhook signature verification — events are recorded but `signature_ok`
  is `false` until per-provider HMAC checks land. The ingestor must not act
  on unverified events.
- Solana memo submission — the anchor service computes the Merkle root and
  persists the `anchors` row, but the on-chain transaction body and signing
  loop is the next piece of work. Verification falls back to "not yet
  anchored" until that lands.
- Plaid `/transactions/sync` polling loop — connection storage is real;
  the worker that polls each Plaid item on a schedule is the next piece.

These are all deliberately structured as "fill in the function body" tasks
rather than "rearchitect the module" — the boundaries and types are stable.
