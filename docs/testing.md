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
| `concurrent_order_db` | **two shoppers racing for the last unit** on separate connections — exactly one wins, the loser gets a clean `Insufficient`, `on_hand` lands at 0, one order exists (the `FOR UPDATE` serialization, which unit logic can't prove) |
| `payment_status_db` | `set_order_payment_status`: `paid_at` stamped once and never moved by a replay; a `Paid` order never regresses (only a refund supersedes); sub-`Paid` transitions still advance |
| `record_payment_db` | `record_payment` money idempotency — dedupe on `UNIQUE (provider, provider_ref)` so a settlement + its ledger post apply once; distinct ref, or same ref under a different provider, is its own payment |
| `payment_events_db` | `record_payment_event` replay dedup — the entire webhook idempotency guarantee (claim once, dedupe retries, distinct per provider) |
| `fulfillment_db` | `record_fulfillment` ships a placed order (shipment + ETA window, order → `fulfilled`), returns `None` for an unknown order, and refuses to ship a `cancelled` order (tx rolls back, no shipment) |
| `recurring_runner_db` | the runner fires an owned recurring order but skips a provider-subscription one |
| `recurring_holds_db` | the runner subtracts live cart holds (skips-but-advances when a shopper holds the stock, fires after expiry) |
| `order_ownership_db` | order items + shipments are scoped to the owner |

There is also a pure-logic host-routing suite that runs in the **default** `cargo
test` (no DB): `tests/host_routing.rs` pins `Config::is_biz_host` /
`host_allowed` — the B2B-chrome authorization gate — against `biz.`-lookalike
spoofs and the `ALLOWED_HOSTS` allowlist.

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

Add as `tests/*_db.rs` (see the pattern above). Now **done**:
`payment_events_db`, `place_order_db` + `concurrent_order_db` (incl. the
concurrent-oversell race), `payment_status_db`, `record_payment_db`,
`fulfillment_db`, and the `is_biz_host` half of the CSRF/host item
(`tests/host_routing.rs`). What remains:

1. **`payments::settle_order` — amount / provider glue.** (a) `charged`
   ≠ `total_cents` → order stays unpaid, no ledger post; (b) provider ≠ the
   order's initiated provider → no-op. The idempotency half is now covered by the
   primitives (`payment_status_db` for the status write, `record_payment_db` for
   the `provider_ref` dedup); this is the private `settle_order` wrapper that
   composes them (needs `SharedState` + a stubbed ledger to test directly).
2. **`/ws` cross-user isolation.** Two authenticated connections; broadcasting
   user B's `cart_id` must **not** push to A. Only the anon-reject is tested
   today (`integration.rs`). Needs an authenticated-ws harness.
3. **`db::run_due_recurring_orders` — due-selection & double-fire.** The provider
   guard (`recurring_runner_db`) and hold-awareness (`recurring_holds_db`) are
   covered; still want: exactly one child + cursor advance for an owned order,
   none for a cancelled/NULL cursor, and one-child-not-two under concurrency (the
   advisory lock).
4. **`security::apply` — remaining CSRF branches.** Header-token *mismatch* (only
   form-field mismatch is tested) and the `PAYLOAD_TOO_LARGE` branch. (The
   `is_biz_host` spoof half is now done in `tests/host_routing.rs`.)
5. **`payments::stripe_webhook` end-to-end replay.** A signed
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
