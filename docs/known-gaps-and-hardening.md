# Known gaps & hardening

Running list of correctness gaps, half-built features, and hardening work,
with file:line evidence and the concrete fix for each. Keep this current as
items are resolved. Companion: [testing.md](testing.md) for the test-coverage
side.

Line numbers drift as the code moves — treat them as a starting point and
re-grep the named function/symbol.

---

## Resolved (keep for context)

- **B2B accounts were dead-on-arrival.** The approval *gate*
  (`is_b2b_approved()` / `customer_profiles.b2b_approved_at`, `db.rs`) blocked
  ordering, the ERP API, checkout, and API keys, but nothing ever *set*
  `b2b_approved_at` — only test fixtures. Fixed by adding the ops action
  `POST /api/v1/ops/customers/{user_id}/approval` + `db::set_b2b_approval`,
  gated by the operations credential.
- **Recurring runner double-fired provider subscriptions.** `place_order` sets
  `next_run_at` for every recurring order, while provider checkout also stands
  up a provider-managed subscription — so both fired each cycle (provider
  charged the card; the internal runner minted a *separate unpaid orphan child
  order* and double-decremented stock). Fixed by guarding
  `db::run_due_recurring_orders` to skip any order with a `payment_subscriptions`
  row. Covered by `tests/recurring_runner_db.rs`.
- **Migration 0006 checksum drift** — see [Migration discipline](#migration-discipline).
- **CI lint was non-blocking.** `fmt` and `clippy` ran with
  `continue-on-error: true`, so style/lint regressions could merge. They are now
  standalone blocking jobs in `.github/workflows/ci.yml` (`cargo fmt --check`, and
  `cargo clippy --locked --all-targets -- -D warnings`), with `actionlint`
  validating both workflow files and every action pinned to a commit SHA.
- **Fiducia leases could not outlive their own TTL.** `run_singleton` acquired a
  120 s lease and ran the job with no renewal, so a tick that took longer than
  its TTL kept working while fiducia reaped the grant and promoted another
  replica — two cross-cluster leaders, silently. The client now implements
  `POST /v1/locks/renew` and heartbeats at TTL/3 for as long as the job runs,
  cancelling the in-flight job when renewal stops proving we hold the grant.
- **A fiducia leader election failed every fail-closed caller.** The client
  treated `307 not_leader` — routine during an election or leadership transfer,
  and explicitly retryable — as a hard rejection, so logins, cart adds, and
  singleton ticks all denied traffic until the election settled. Commands now
  retry the configured base URL with backoff (never the `Location` header: this
  client attaches a bearer credential to every request).
- **Login/IP throttles were replica-local and trusted any `X-Forwarded-For`.**
  They now use the modular `rate_limit` service (Fiducia-backed and fail-closed
  when configured), and `request_trust` accepts forwarding headers only from
  configured direct-proxy CIDRs.
- **Magic links could silently create accounts and email-spam an arbitrary
  address.** Existing-account login now sends `create_user: false`; self-signup
  requires explicit enablement plus a successful server-side Turnstile proof.
- **MFA and CSRF browser state was mutable.** The pending SMS challenge is a
  short-lived HMAC-signed, user-bound cookie, while the CSRF cookie is now
  HttpOnly and its synchronizer token is supplied through the rendered DOM.
- **AAL was read from an unverified JWT payload.** GoTrue's authenticated
  `/auth/v1/factors` response supplies the assurance level (`current_level`).
- **Second-factor *verification* was unthrottled.** `mfa-send` was rate limited
  from the start, but `POST /login/2fa` — and the enrollment verifiers
  `/account/2fa/totp/verify` + `/account/2fa/phone/verify`, which also mint an
  AAL2 session — accepted unlimited code submissions. Reaching them needs only a
  live AAL1 session, and the factor is a 6–8 digit code, so the attempt budget
  was the only thing standing between a held magic-link session and a ~10⁶
  guessing exercise (GoTrue's own limits were load-bearing but undocumented and
  outside our control). All three now share the `mfa-verify` bucket
  (`auth::MFA_VERIFY_MAX_ATTEMPTS` per `MFA_VERIFY_WINDOW`, per user), with a
  compile-time assertion that the budget cannot be loosened to the point where
  a 6-digit keyspace is exhaustible.
- **Order/payment endpoints are now rate limited.** `/checkout` and
  `/orders/{id}/pay` share a per-user `checkout-user` budget (they decrement
  stock and open provider payment sessions), and `/pay/success` has a per-IP
  `pay-return-ip` budget. The last one matters because it is unauthenticated by
  necessity and every branch issues an outbound provider API call with a
  caller-supplied session/order id *before* confirming the order exists — an
  unthrottled amplifier into our own provider rate limits and bills.
- **A webhook with no event id collapsed the replay-dedup bucket.** All three
  handlers read the provider event id with `unwrap_or_default()`, and
  `(provider, "")` is a perfectly usable `payment_events` key — so the first
  id-less event claimed it and every later id-less event was discarded as an
  already-processed replay (200, never settled). The id now goes through
  `event_id_of`, which rejects absent/blank/non-string ids with a 400.
- **Latent request-path panics on the API auth guard are gone** (was gap #7).
  `api::orders_list` / `api::orders_create` re-derive the pool with a
  `let ... else` returning 503 instead of `.expect("authenticate checked pool")`.
- **2FA enforcement no longer depends on one endpoint's response shape.** The
  enrolled-factor list is now taken from BOTH the authoritative
  `/auth/v1/user` object and `/auth/v1/factors`, unioned by id. Previously a
  `200` from `/auth/v1/factors` whose `factors` key was absent or renamed
  emptied the list, so `needs_aal2()` silently went false for every enrolled
  user — a fail-open 2FA bypass. A verified factor must now be missing from
  both sources to be overlooked, while a genuine no-MFA user (empty in both)
  still needs no step-up, so the change cannot lock anyone out. AAL defaults
  to the conservative `aal1` when `current_level` is absent.

---

## Open functional gaps (ranked by impact)

### 1. Subscriptions cannot be cancelled in-app; provider cancel doesn't stop the internal runner
- No subscription/cancel route exists in `lib.rs`'s `router()`. `next_run_at`
  is only ever *set* at checkout and *advanced* by the runner — never cleared.
- The UI advertises the subscription and its next run (`orders.rs`) but renders
  **no cancel/pause control**.
- `db::set_subscription_status(Cancelled)` is reachable only from provider
  webhooks; it updates `payment_subscriptions.status` and does **not** touch the
  parent order's `next_run_at`/`status`. (After the double-fire fix the internal
  runner already skips provider-backed orders, so a provider cancel no longer
  leaves an internal loop — but an *internally-driven* Net-30 recurring order
  still has no cancel path at all.)
- **Fix:** add `POST /orders/{id}/cancel` (customer) + an ops equivalent that
  sets `status='cancelled'` and `next_run_at=NULL`, calls the provider's
  cancel API when a `payment_subscriptions` row exists, and renders a
  cancel/pause control in the orders UI. Wire it into `router()`.

### 2. Order cancellation is entirely absent — `OrderStatus::Cancelled` is a write-never state
- `Cancelled` is only ever *read*: CSS class, payment-badge mapping,
  `payment_retryable` guard, the B2B "Order management" **status filter offers
  it**, and raw SQL `status <> 'cancelled'`. No code path ever *writes*
  `'cancelled'`, so the filter option matches nothing and `payment_retryable`'s
  `status != Cancelled` check is permanently true.
- **Fix:** a cancel action (customer while unfulfilled, ops any time) that
  writes `status='cancelled'`, restores stock (`inventory.on_hand += qty`), and
  triggers a refund when already paid (see #3).

### 3. Refunds are scaffolded but unbuilt
- `PaymentStatus::Refunded`, `PaymentKind::Refund` exist and are encoding-tested;
  `payment_class` renders Refunded; `dollars()` handles negatives. But **none of
  the three webhook handlers has a refund branch** (Stripe `charge.refunded`,
  PayPal `PAYMENT.CAPTURE.REFUNDED`, Square `refund.*`), and there is no ops
  refund endpoint. Nothing ever produces `Refunded`/`Refund`.
- **Fix:** add the refund webhook branches (write a `PaymentKind::Refund`
  payment row + set order `payment_status='refunded'`) and an ops refund
  endpoint that calls the provider refund API. Post the reversal to the Quaestor
  ledger the same way settlement does.

### 4. Internal recurring runner produces unpaid children (for the orders it *does* own)
- After the provider-subscription guard, the runner only fires recurring orders
  with **no** provider subscription (Net-30 / unpaid recurring). It still inserts
  child orders with `payment_status='pending'` and **no payment/invoice step** —
  `stripe_net30_invoice` is only called from checkout, never from the runner.
- **Fix:** have the runner initiate the per-cycle invoice/charge for the orders
  it owns (call the Net-30 invoice path for B2B, or the configured provider),
  and notify the customer.

### 5. `OrderStatus::Processing` is unreachable
- Orders default to `'placed'`; `record_fulfillment` writes `'fulfilled'`
  directly. Nothing writes `'processing'`, yet it's offered in the B2B filter
  and given a CSS class.
- **Fix:** either drop `Processing` from the enum/filter, or write it when a
  payment settles / an order is picked but not yet shipped.

### 6. (Minor) PayPal subscription order marked Paid on activation with no amount check
- `BILLING.SUBSCRIPTION.ACTIVATED` calls `settle_order(..., charged=None)`, so
  the order flips to `Paid` before any money is verified (inconsistent with the
  return path, which sets `Processing`). `settle_order` skips `charge_matches`
  when `charged` is `None`.
- **Fix:** treat activation as `Processing`/subscription-active, and only mark
  `Paid` on the first captured cycle payment with a verified amount.

---

### 6b. Order/checkout coordination gaps (from the fiducia audit)

Ranked; all still open. The storefront cart path is guarded by the cart-row
`FOR UPDATE` in `place_order` (`db.rs`) — these are the paths it does not cover.

- **The ERP API order path has no idempotency at all.** ([#2](https://github.com/athlet-o/athleto-app-rs/issues/2)) `POST /api/v1/orders`
  calls `place_order` with `cart_id = None`, so it takes no cart lock, and
  `OrderRequest` (`athleto-interfaces/schema/athleto.schema.json`) carries no
  idempotency key. An ERP client that retries on timeout gets a second order and
  a second stock decrement. **Fix:** accept an `Idempotency-Key` header, and add
  an `order_idempotency (user_id, key) → order_id` table with a unique
  constraint to the declarative schema; return the original order on replay. A
  fiducia lease keyed on `athleto:order:{user}:{key}` is the right *complement*
  (it collapses the concurrent case cheaply) but must not be the only guard — a
  lease is liveness-coupled and cannot survive the claiming process dying.
- **Webhook claims commit before the work they guard.** All three handlers call
  `db::record_payment_event` (a correct `INSERT … ON CONFLICT DO NOTHING` on the
  `(provider, event_id)` PK) on the bare connection, so the claim commits *first*.
  If the handler then errors or the replica dies, it returns 500 with the claim
  already durable, and the provider's retry is swallowed as a replay — the order
  never settles. **Fix:** open one transaction spanning claim + settlement so a
  rollback un-claims the event. This is the correct layer; a lease does not fix
  it.
- **Settlement is three independent autocommit writes** (`set_order_payment_status`,
  `record_payment`, `upsert_subscription`) with no transaction, so a crash
  mid-settlement leaves an order marked paid with no `payments` row. Same fix as
  above.
- **Checkout reads cart lines outside the order transaction.** `orders::checkout`
  calls `db::cart_lines` before `place_order` opens its tx, and passes the
  resulting quantities *and unit prices* in. An add that commits in that window
  is silently dropped by the cart clear, and a price change in that window is
  never re-validated. **Fix:** re-read the lines inside the transaction, after
  the cart `FOR UPDATE`, and price from `products` there.
- ~~**`/checkout` and `/orders/{id}/pay` are not rate limited.**~~ Resolved:
  both share the per-user `checkout-user` budget, and `/pay/success` gained a
  per-IP `pay-return-ip` budget. See Resolved above.
- **PayPal capture is issued with no already-paid check and no
  `PayPal-Request-Id`.** PayPal rejects the second capture, so a real double
  charge is unlikely, but the rejection path is what regressed order status
  (fixed above).
- **Stock is decremented at checkout and never restored.** There is no
  `on_hand = on_hand + n` anywhere in `src/`, so an abandoned checkout, a
  `/pay/cancel`, or a `Failed` webhook permanently consumes inventory. Ties into
  gaps #2 and #3 (cancellation and refunds).

## Robustness & infra gaps (non-functional)

### 8. The `@athleto/sync` local-first SDK is not wired in
- No references to `@athleto/sync`, the `athleto-optimistic` htmx extension, or an
  `/api/sync` catch-up endpoint anywhere in `src/`. The cart uses the app's own `/ws`
  (`src/ws.rs`) with the **stock** htmx ws extension (`src/assets.rs`), not the SDK's
  optimistic IndexedDB client — so the offline/optimistic sync layer in the
  `athleto-sync` repo is currently unused here.
- **Fix (decision):** either adopt it (mount `startSync` +
  `registerOptimisticExtension` on the cart, add `/api/sync` + Postgres
  `version`/`sync_sequence` columns — see `athleto-sync/docs/adoption.md`), or document
  that the bespoke `/ws` is the intended path and the SDK is out of scope here. Don't
  leave it ambiguous.

### 9. One transitive advisory
- `cargo audit`: `RUSTSEC-2023-0071` (rsa 0.9 "Marvin Attack" timing sidechannel,
  medium) via the SQLx/SeaORM MySQL stack — no upstream fix. If the MySQL driver is
  unused, disabling that SeaORM feature removes the `rsa` dependency; otherwise track.
  Plus `proc-macro-error2` unmaintained (warning).

### 10. nginx `/jello` gateway not yet cut over to this app
- `~/codes/ores/k8s-cluster/remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml`:
  `location = /jello` and `/jello/sample` still set their upstream (`$dd_up_3` /
  `$dd_up_4`) to `dd-remote-web-home.default.svc.cluster.local:8080`. Athleto is
  reachable only via its dedicated Ingress (`jello-ws:8145`).
- **Fix:** repoint those upstreams to `jello-ws.default.svc.cluster.local:8145` and
  update the guard in `remote/tests/general/athleto-app-config.test.ts` (which asserts
  the old wiring on purpose).

---

## Migration discipline

The numbered files under `migrations/` are a frozen history of the original
Supabase rollout. A historical checksum edit in `0006_payments.sql` prevented
`0007` and `0008` from applying and caused runtime column drift; do not edit or
extend that trail for production schema changes.

- The current source of truth is
  `k8s-cluster/remote/libs/pg-defs/schema/databases/athleto/schema.sql`, targeting
  the dedicated `athleto` database.
- Change the declarative contract, then use `scripts/dpm.sh diff`, `verify`, and
  `review`. `apply` is an explicit human-reviewed release action.
- The application never runs DDL at boot. This avoids startup races, pooler
  advisory-lock timeouts, and one replica migrating beneath another.
- CI may apply the frozen files to create an isolated compatibility fixture;
  that does not make them the production migration authority.

---

## Hardening posture already in place (don't regress)

- **CSRF**: HttpOnly synchronizer token on every state-changing form + htmx header;
  `/api/v1` is the only exemption (bearer-auth, no ambient cookie); webhooks are
  exempt but verify provider signatures. Constant-time token compare.
- **Payment webhooks fail closed**: no signing secret ⇒ reject; HMAC over the
  **raw** body; replay-deduped via `payment_events (provider, event_id)`.
- **Host allowlist** (`ALLOWED_HOSTS`) for auth-redirect bases + biz-host chrome;
  permissive-with-warning when unset.
- **Login-flow pinning**: `athleto_login_flow` cookie must match the `flow` param
  on confirm, so a leaked magic link can't be completed in another browser.
- **B2B requires approval *then* 2FA** before ordering/API (`require_b2b_ready`).
- **B2B host selection is presentation-only**; authorization continues to
  require `CustomerProfile::is_b2b_approved()` and the verified-MFA gate.
