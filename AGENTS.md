# AGENTS.md — athleto-app-rs

Rules of the road for agents (and humans) working in this repo.

## Stack

Maud + Axum + SeaORM + Supabase + HTMX. Serves two storefronts from one
binary by Host header: app.athleto.store (B2C) and biz.athleto.store (B2B).
Boots degraded with zero secrets — every new feature must keep that property
(missing config ⇒ "not configured" notice, never a crash).

## Data access: SeaORM only

- Entity queries use SeaORM models in `src/entities/` and the
  `sea_orm::DatabaseConnection` stored in `AppState::pool`.
- Locking-heavy transactional queries may stay hand-written, but execute them
  through `sea_orm::Statement`, `ConnectionTrait`, and `TransactionTrait`.
- Do not add a direct `sqlx` dependency or call SQLx APIs. SeaORM's Postgres
  driver is an internal implementation detail, not an application data layer.

## Migrations

- The numbered SQL files in `migrations/` are a frozen audit trail. Runtime
  code never applies DDL or migrations at boot. The schema authority is the
  declarative `athleto` database contract in `k8s-cluster`'s `pg-defs`, using
  [dpm](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)
  (org: [github.com/declarative-migrations](https://github.com/declarative-migrations)).
  Edit the declarative schema, let the database converge onto it, and review
  the SQL dpm emits. Install: `brew install
  declarative-migrations/tap/dpm` (Linux: `scripts/install.sh` in the dpm
  repo). See the billing-server repo for the finished pattern (`schema/` +
  `scripts/dpm.sh`, `migrations/` frozen as an audit trail).
- **RDS namespace rule:** the contract targets its **own database named
  `athleto`** on the shared dd-platform Amazon RDS instance —
  one database per project, never a shared `public` schema, so our table
  names can't collide with other projects.
- The schema authority is `pg-defs/schema/databases/athleto/schema.sql` in
  `k8s-libs-and-shared-defs` (local checkout:
  `~/codes/ores/k8s-cluster/remote/libs/pg-defs`).

## Payments

- Providers: Stripe (cards, B2B ACH, Net-30 hosted invoices), PayPal
  (orders + subscriptions), Square (payment links + subscription plans). All
  hosted/redirect — no PAN ever touches this server.
- Config is env-driven and each provider is independently optional
  (`ATHLETO_STRIPE_*`, `ATHLETO_PAYPAL_*`, `ATHLETO_SQUARE_*`; see README).
  Placeholder values live in GitHub Actions secrets; real values in
  `~/.config/athlet-o/secrets.env` locally and an external Vault/cloud secret
  store injected through ESO/Secrets Store CSI in clusters.
- **Secrets sourcing** goes through `src/secrets.rs`: env first, fiducia
  config KV (`secrets/athleto/<ENV_NAME>`) as the cross-provider overlay for
  gaps. New secret env vars must be added to `secrets::MANAGED_KEYS` (an
  explicit allowlist — never widen it to arbitrary names) and to the README
  table + CI workflow. Fiducia KV may protect values with a versioned local
  keyring or Vault Transit; explicit plaintext entries and legacy client-side
  envelopes are migration modes. See docs/secrets-management.md.
- Webhooks must stay idempotent: every handler records
  `(provider, event_id)` in `payment_events` first and bails on replay.
  Ledger postings use idempotency keys (`athleto:order:…`,
  `athleto:payment:…`).
- The Quaestor billing-server (`ATHLETO_BILLING_*`) is an **observer** ledger
  (Model A): post AR/payment transactions, read billing-state; never treat it
  as the payment rail. Writes are fire-and-forget — the ledger being down must
  never block checkout.

## Testing

`cargo test` must pass with no database and no network. Pure-logic tests live
in each module (`#[cfg(test)]`); HTTP/provider calls are not unit-tested here.

## Syncing with the remote

"Sync with the remote" (or just "sync") is a **two-way** exchange — pull the
remote's commits down **and** push yours up. It is never push-only, and a clean
local tree does not by itself mean "synced": you are done only once local and
the remote hold the same commits.

To sync:

1. **Commit your work first** (`git add` + `git commit`) so the tree is clean —
   pull/merge only into a clean tree. `git pull` / `git merge` aborts when an
   incoming change touches a file you have edited, and even when it doesn't it
   buries the merge in your uncommitted work. (Can't commit yet? `git stash`,
   then `git stash pop` after step 3.)
2. `git fetch --all --prune` — safe any time; it only updates tracking refs.
3. `git pull` (fetch + merge) — or `git merge` the upstream branch — to
   integrate the remote's commits.
4. `git push` to publish yours.

Integrate with **`git merge` / `git pull`**. **Never `git rebase` to sync** — it
rewrites history and breaks shared branches.
