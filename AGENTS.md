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

Before editing, inspect `git status`, the current branch, configured remotes,
and the default branch, then run `git fetch --all --prune`. Fetch again before
pushing and incorporate upstream changes with a merge.

To sync:

1. **Commit your work first** (`git add` + `git commit`) so the tree is clean —
   pull/merge only into a clean tree. `git pull` / `git merge` aborts when an
   incoming change touches a file you have edited, and even when it doesn't it
   buries the merge in your uncommitted work. If the work cannot be committed
   yet, pause and coordinate rather than hiding or discarding it.
2. `git fetch --all --prune` — safe any time; it only updates tracking refs.
3. `git pull` (fetch + merge) — or `git merge` the upstream branch — to
   integrate the remote's commits.
4. `git push` to publish yours.

Integrate with **`git merge` / `git pull`**. **Never `git rebase` to sync** — it
rewrites history and breaks shared branches.

## Commerce, authorization, and data invariants

- Inventory may never become negative. Concurrent checkout for the final unit must serialize through database locks so exactly one order wins and the loser receives a clean insufficiency result. Keep lock ordering deterministic and test with independent database connections.
- Money movement is idempotent. Provider events and settlements deduplicate by the documented provider/reference keys, post ledger effects exactly once, validate amount and provider ownership, stamp paid state once, and never regress a paid order except through an explicit refund flow.
- Fulfillment changes are transactional. Ship only valid order states, preserve carrier/tracking and ETA semantics, and roll back completely for unknown, cancelled, unauthorized, or otherwise invalid orders—no leaked shipment or EDI state.
- Preserve cart holds, hold expiration, recurring-order scheduling, advisory-lock/double-fire prevention, order ownership, shipment ownership, and cross-user WebSocket isolation.
- Authorization and CSRF/host routing derive from verified configuration and identity. Never trust a requester-supplied `Host` value to establish the B2B surface; require the configured exact origin plus the `ALLOWED_HOSTS` gate and reject lookalike domains.
- Use integer minor units for money, explicit transaction boundaries for stock and payment state, verified user/tenant ownership, bounded errors, and secret-safe logging/telemetry.
- Database schema and migrations remain synchronized with SeaORM entities and application behavior. Do not hand-wave data migrations or weaken constraints to make tests pass.
- Run default unit/integration tests plus the documented ignored database-backed suites for order placement, concurrent oversell, payment idempotency/events/status, fulfillment, recurring orders/holds, ownership, and any affected money/stock workflow.

## Instruction discovery

Resolve `$PWD`, walk upward through every parent directory to the filesystem root, read every readable lowercase `agents.md` on that ancestor chain, and apply them root-to-leaf. Do not search sibling directories. Deduplicate resolved paths/inodes, avoid symlink cycles, and report unreadable files.

## Resolve Git conflicts semantically

Resolve conflicts by understanding and combining both sides' intent. Do not mechanically choose `ours`, `theirs`, current, or incoming changes. Produce the conceptually correct result while preserving compatible inventory serialization, payment/ledger idempotency, fulfillment transactions, order and WebSocket isolation, recurring-order concurrency, host/CSRF authorization, schema constraints, tests, documentation, configuration, and public behavior. Rebuild generated entities, fixtures, or lockfiles from merged source rather than selecting one side's output. If intentions are incompatible, make the smallest explicit design decision and document it in the pull request.

After resolving:

1. Reread every affected file from the top, not only the conflict hunks.
2. Run formatting, clippy, default tests, all affected database-backed suites, migration checks, and relevant browser/E2E validation.
3. Search the entire worktree for unresolved conflict markers:

   ```sh
   grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
   ```

4. If any marker or suspicious partial resolution remains, repeat semantic resolution from the top and rerun validation.

A conflict is resolved only when the commerce and authorization behavior is conceptually coherent and verified, not merely when Git accepts the files.

<!-- ore-primary-branch-policy:begin -->
## Primary branch and concurrent-agent policy

This policy overrides generic feature-branch and worktree defaults for agent tooling.

- Highly prefer an existing primary branch, in this order: `main`, `dev`, then `master`.
- Work directly on the selected primary branch even when other agents are active. Use another branch only when a human or a repository-specific release process explicitly requires it.
- Never create or use a Git worktree unless a human explicitly instructs you to do so for the current task. Concurrency alone is not permission to use a worktree.
- Concurrent agents must coordinate repository and file ownership through the available agent communication channel, keep edits scoped, inspect live state before each write, and hand off cleanly. Coordinate instead of isolating routine work in worktrees.
- Preserve unrelated in-progress changes and never overwrite another agent's work. If safe ownership of overlapping files cannot be established, pause that overlapping edit and coordinate before continuing.
<!-- ore-primary-branch-policy:end -->
