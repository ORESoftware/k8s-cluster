# Testing

How the suites are structured, what's covered, the biggest gap, and how to shore
it up. Companion: [known-gaps-and-hardening.md](known-gaps-and-hardening.md).

## Layers

| Layer | Where | Runs | Covers |
| --- | --- | --- | --- |
| Rust unit | `#[cfg(test)]` in `src/*.rs` | `cargo test` | pure helpers: enums, `charge_matches`, `decimal_to_cents`, `dollars`, CSRF token compare, signature verifiers, hashing, ETA/tracking, ship-method math |
| Rust integration | `tests/integration.rs` | `cargo test` | drives the real `router()` in **degraded mode** (`AppState::new(None, …)` — no DB): routing, CSRF branches, security headers, rate limiting, host allowlist, `/ws` anon-reject |
| Rust DB-backed | `tests/*_db.rs`, `#[ignore]` | `DATABASE_URL=... cargo test -- --ignored` | the money/stock paths a degraded run can't reach; **wired into the e2e CI job** which has a Postgres |
| Browser E2E | `e2e/*.test.mjs`, `node:test` | `cd e2e && npm test` | the real app in Chrome under **both Playwright and Puppeteer** |
| Cluster smoke | `e2e/cluster/run-cluster.mjs` | opt-in CronJob | live storefront scenarios via `dd-browser-test-server` (both engines) |

## DB-mutation coverage

Historically **every** Rust test ran degraded or against pure helpers — *no* test
exercised a real DB mutation. That gap is now substantially closed; the
DB-backed suites (all `#[ignore]`, run in the e2e CI job against its Postgres,
each seeding its own rows and cleaning up) cover:

| Suite | Covers |
| --- | --- |
| `place_order_db` | stock decrement on success + cart/hold clearing; a live cross-cart hold blocking oversell (`Insufficient`, no decrement); `ensure_hold` cross-cart availability, lazy expiry, and upsert-not-stack |
| `payment_status_db` | `set_order_payment_status`: `paid_at` stamped once and never moved by a replay; a `Paid` order never regresses (only a refund supersedes); sub-`Paid` transitions still advance |
| `payment_events_db` | `record_payment_event` replay dedup — the entire webhook idempotency guarantee (claim once, dedupe retries, distinct per provider) |
| `recurring_runner_db` | the runner fires an owned recurring order but skips a provider-subscription one |
| `recurring_holds_db` | the runner subtracts live cart holds (skips-but-advances when a shopper holds the stock, fires after expiry) |
| `order_ownership_db` | order items + shipments are scoped to the owner |

**New money/stock logic should still get a DB-backed test** — see the pattern below.

### Adding a DB-backed test (the pattern)

`tests/recurring_runner_db.rs` is the template:

```rust
#[tokio::test]
#[ignore] // needs a real DATABASE_URL; run with --ignored
async fn my_invariant() {
    let conn = db::build_pool(&std::env::var("DATABASE_URL").unwrap()).await.unwrap();
    // seed with sea_orm::Statement raw SQL, call the db:: fn, assert via SELECT,
    // then DELETE your rows (use a fresh Uuid namespace so parallel runs don't collide).
}
```

- `#[ignore]` keeps them out of the default `cargo test` (which has no DB).
- The **e2e CI workflow** (`.github/workflows/e2e.yml`) runs them with
  `cargo test --test <name> -- --ignored` against its throwaway Postgres after
  the declarative schema has been applied. Add each new `*_db.rs` there.
- Migrations only need Supabase's `auth.uid()` stubbed (the workflow does this);
  everything else is vanilla Postgres.

## Running the suites

```sh
# Rust (degraded — no DB, no network):
cargo test

# Rust DB-backed (against a real DB):
DATABASE_URL=postgres://… cargo test --test recurring_runner_db -- --ignored --nocapture

# Browser E2E, one engine, against a locally-booted app on :8145:
#   export SUPABASE_URL / SUPABASE_SERVICE_KEY (for authed suites) and
#   ATHLETO_OPERATIONS_API_KEY / E2E_OPS_KEY (for the ops-approval test)
cd e2e && npm install
E2E_ENGINE=playwright node --test --test-timeout=60000 --test-concurrency=1 *.test.mjs
E2E_ENGINE=puppeteer  node --test --test-timeout=60000 --test-concurrency=1 *.test.mjs
npm test          # both engines

# Cluster live smoke (needs dd-browser-test-server healthy):
BROWSER_TEST_URL=… SERVER_AUTH_SECRET=… node e2e/cluster/run-cluster.mjs
```

The browser harness (`e2e/lib/`) drives one shared driver interface across both
engines, logs in hermetically (Supabase admin magic-link + a self-set
`athleto_login_flow` cookie — no email send), and has an RFC-6238 `totp()`.
Auth-dependent suites self-skip when `SUPABASE_*` is unset, so CI stays green
without secrets and richer with them. `E2E_SKIP_LIVE=1` skips the biz-host check
that hits the deployed site.

The **ERP `/api/v1` surface** has two suites: `erp-api-guard.test.mjs` asserts
the negative space — a missing/malformed/unknown key never yields a `2xx` — and
needs no secrets, so it runs in every lane (with a DB it's a `401`; degraded
it's the `503` the guard returns before reading the token). The write path,
`erp-orders-api.test.mjs`, needs a real approved-B2B key: minting one through the
UI requires an AAL2 session (`require_b2b_ready` + `require_full`), which the
harness doesn't automate, so it takes a pre-issued key from **`E2E_ERP_KEY`** and
skips when unset. Its last case — a retried create with an `Idempotency-Key`
returning the original order — stays `skip`ped against
[athleto-app-rs#2](https://github.com/athlet-o/athleto-app-rs/issues/2) until
that guard lands; flip it on when it does.

## Highest-value missing tests (ranked)

All DB-bound; add as `tests/*_db.rs` (see the pattern above). Items 1, 3, and 4
from the original list are now **done** (`payment_events_db`, `place_order_db`);
what remains:

1. **`payments::settle_order` — amount / provider / idempotency.** (a) `charged`
   ≠ `total_cents` → order stays unpaid, no ledger post; (b) provider ≠ the
   order's initiated provider → no-op; (c) settling twice with the same
   `provider_ref` → `Paid` once, ledger posted once. (The status-write half —
   `paid_at` idempotency + the no-regression guard — is now covered by
   `payment_status_db`; this is the `settle_order` wrapper around it.)
2. **`db::place_order` — concurrent oversell.** The decrement, cross-cart-hold
   `Insufficient`, and cart-clear cases are covered by `place_order_db`; the
   remaining gap is (c) two *concurrent* `place_order` on one product not
   overselling under the `FOR UPDATE` — needs two live connections racing, which
   the single-connection seed pattern can't express.
3. **`/ws` cross-user isolation.** Two authenticated connections; broadcasting
   user B's `cart_id` must **not** push to A. Only the anon-reject is tested
   today (`integration.rs`). Needs an authenticated-ws harness.
4. **`db::run_due_recurring_orders` — due-selection & double-fire.** The provider
   guard (`recurring_runner_db`) and hold-awareness (`recurring_holds_db`) are
   covered; still want: exactly one child + cursor advance for an owned order,
   none for a cancelled/NULL cursor, and one-child-not-two under concurrency (the
   advisory lock).
5. **`security::apply` — untested CSRF branches.** Header-token *mismatch* (only
   form-field mismatch is tested) and the `PAYLOAD_TOO_LARGE` branch; plus
   `Config::is_biz_host` rejecting a spoofed `biz.`-looking Host.
6. **`db::record_fulfillment` — status advance & scoping.** Correct ETA window,
   flips order to `fulfilled`, `None` for an unknown order.
7. **`payments::stripe_webhook` end-to-end replay.** A signed
   `checkout.session.completed` settles once; the identical event replayed
   returns 200 with no re-settle and no second ledger post (ties the
   `record_payment_event` dedup and `settle_order` idempotency together at the
   handler boundary).

**Already well covered — don't re-add:** signature verification incl. replay
window (all three providers), CSRF missing/mismatched/partial + security headers
+ nonce freshness + rate limiting, `host_allowed`, and the browser flows
(storefront/cart/holds/checkout/receipt/tracking/reorder/2FA-setup/B2B-approval/
payment-status), the ERP `/api/v1` auth guard (missing/malformed/unknown key,
guest-safe, always-on), cross-user order-read isolation (`order-isolation`), the
API-key card + its 2FA mint gate (`account-api-keys`), the empty-cart/reorder
checkout edges (`checkout-flows`), and the ERP order-create contract +
validation (`erp-orders-api`, `E2E_ERP_KEY`-gated), plus the DB-mutation suites
tabled above (stock/oversell/holds, payment-status regression guard + `paid_at`
idempotency, webhook replay dedup, recurring runner provider + hold guards).
