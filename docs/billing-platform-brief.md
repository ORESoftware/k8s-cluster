# Billing Platform — Product Brief

> Multi‑tenant AR/AP ledger that syncs with every payment platform your customers
> actually use — Stripe, PayPal, Braintree, Coinbase, banks, on‑chain — and
> answers the two questions every finance team asks every day.

---

## The two questions

Every CFO, controller, AR clerk, and AP clerk wakes up trying to answer exactly
two questions. Everything else is plumbing in service of these:

1. **When do I bill the customer, and for how much?**
2. **When do I pay a vendor, and how much?**

Today they answer these by stitching together Stripe dashboards, PayPal CSVs,
QuickBooks exports, spreadsheets emailed at 9pm, and Slack messages from the
ops team. The answers are stale, partial, and frequently wrong.

We make these two questions a single API call (and a single dashboard) backed
by a continuously reconciled ledger.

---

## The answer: one ledger per entity, synced everywhere

For every tenant we maintain a **per‑entity double‑entry ledger** keyed by
`customer_id` and `vendor_id`. Every dollar (or USDC, or SOL) that moves
anywhere your business touches lands as a posting in that ledger within
seconds — and reconcilers prove, on a schedule, that what the ledger says
matches what each external system says.

```
                ┌──────────────────────────────────────────────────────┐
                │  Per‑tenant ledger (double‑entry, append‑only)       │
                │                                                      │
                │   accounts_receivable / <customer_id>                │
                │   accounts_payable    / <vendor_id>                  │
                │   clearing            / <provider>                   │
                │   cash                / <bank_account>               │
                │   onchain             / <wallet>                     │
                │   revenue, fees, chargebacks, unallocated_cash, …    │
                └──────────────────────────────────────────────────────┘
                       ▲                                    ▲
                       │ postings                           │ proofs
        ┌──────────────┴──────────────┐      ┌──────────────┴──────────────┐
        │   Ingestors (webhooks +     │      │   Reconcilers (scheduled)   │
        │   polling, per provider)    │      │   diff ledger vs provider   │
        │                             │      │   write breaks to dashboard │
        │   Stripe, Braintree,        │      │                             │
        │   PayPal, Coinbase,         │      │   balance_transactions,     │
        │   Plaid banks, Solana,      │      │   settlement reports,       │
        │   ACH/wire feeds, …         │      │   chain finality, …         │
        └─────────────────────────────┘      └─────────────────────────────┘
```

The ledger is the source of truth. Providers are upstream noise that we
normalize, post, and continuously prove against.

---

## Answering Question 1 — "When do I bill the customer, and for how much?"

For any customer, at any moment, we can return:

```http
GET /v1/customers/{customer_id}/billing-state

{
  "customer_id": "cus_8f2c…",
  "as_of": "2026-05-17T22:55:00Z",
  "currency": "USD",

  "outstanding_balance_minor": 482300,        // they owe you $4,823.00
  "next_bill": {
    "due_on": "2026-06-01",
    "amount_minor": 199900,                   // $1,999.00
    "reason": "subscription_renewal",
    "source_subscription_id": "sub_…"
  },
  "aging": {
    "current":     199900,
    "1_30_days":   149400,
    "31_60_days":  133000,
    "61_90_days":       0,
    "over_90":          0
  },
  "credit_memos_minor": 0,
  "unallocated_cash_minor": 0,
  "last_payment": {
    "received_on": "2026-04-29",
    "amount_minor": 199900,
    "via": "stripe_card",
    "external_id": "pi_3O…"
  },
  "reconciliation_status": "clean",           // or "breaks_open"
  "as_of_confidence": "finalized"             // vs "pending"
}
```

Behind the scenes that single response is built from postings touching:

- `accounts_receivable/<customer_id>` — every invoice issued
- `clearing/stripe`, `clearing/paypal`, `clearing/coinbase` — payments received,
  not yet settled to a bank
- `cash/<bank_account>` — payments settled to your bank
- `unallocated_cash/<customer_id>` — money received but not yet matched to an
  invoice (the silent killer of every billing system)
- `credit_memos/<customer_id>` — refunds and adjustments owed back

The `next_bill` is computed from subscription schedules, usage meters, and
manual invoice drafts — all of which post into AR the moment they're committed.

### What this lets the customer do

- **Dunning:** "Show me every customer with anything in `61_90_days` and email
  them today." One query.
- **Revenue forecasting:** "What will I bill next 30 days?" Sum of
  `next_bill.amount_minor` across all customers.
- **Cash forecasting:** "Of what I bill, what will actually land in my bank,
  and when, after Stripe's 2‑day delay and PayPal's 3‑day hold?" Built into
  the projection because we know each provider's settlement timing.
- **Disputes / chargebacks:** A Stripe chargeback webhook posts a reversal
  against the original invoice within seconds, and the customer's balance
  reflects it before the support ticket lands.

---

## Answering Question 2 — "When do I pay a vendor, and how much?"

Symmetric API, AP side:

```http
GET /v1/vendors/{vendor_id}/payable-state

{
  "vendor_id": "ven_91ab…",
  "as_of": "2026-05-17T22:55:00Z",
  "currency": "USD",

  "outstanding_payable_minor": 1284500,       // you owe them $12,845.00
  "next_payment": {
    "due_on": "2026-05-20",
    "amount_minor": 750000,                   // $7,500.00 — pay in 3 days
    "preferred_rail": "ach",                  // cheapest viable rail
    "rail_options": [
      { "rail": "ach",     "fee_minor": 25,    "eta_business_days": 2 },
      { "rail": "wire",    "fee_minor": 1500,  "eta_business_days": 0 },
      { "rail": "paypal",  "fee_minor": 22500, "eta_business_days": 0 },
      { "rail": "usdc_sol","fee_minor": 1,     "eta_business_days": 0 }
    ],
    "source_bill_ids": ["bill_…", "bill_…"]
  },
  "aging": {
    "due_now":    750000,
    "due_7_days": 184500,
    "due_30_days": 350000,
    "overdue":         0
  },
  "approval_state": "approved",               // or "pending_approver"
  "duplicate_risk": "none",                   // we de‑dupe across rails
  "vendor_payout_methods": ["ach", "paypal", "usdc_sol"]
}
```

Behind the scenes:

- `accounts_payable/<vendor_id>` — every bill received and approved
- `clearing/<rail>` — payments initiated, not yet settled
- `cash/<bank_account>` — bank balance available to pay from
- `payout_methods/<vendor_id>` — the rails this vendor accepts, with on‑file
  account details (encrypted per‑tenant)

The `preferred_rail` is computed from: vendor‑accepted rails ∩ tenant‑enabled
rails, optimized for (fee, settlement time, your bank's available balance).

### What this lets the customer do

- **Pay run:** "Show me everything due in the next 7 days, grouped by
  preferred rail, and let me approve the batch." One screen.
- **Cash management:** "Do I have enough in my operating account to cover
  payroll Friday after these AP runs clear?" Real, because we know rail
  settlement timing.
- **Duplicate prevention:** A vendor sends the same invoice via email and a
  portal — we hash the invoice + amount + date + vendor and refuse to post the
  second one. (This is the single biggest source of AP loss at scale.)
- **Multi‑rail vendor payouts:** A vendor in Argentina takes USDC, a vendor in
  Germany takes SEPA, a vendor in the US takes ACH. One API call per vendor;
  we pick the rail and execute.

---

## What we sync, and from where

| Provider                | Direction | Truth source we reconcile against              | Notes |
|-------------------------|-----------|------------------------------------------------|-------|
| **Stripe**              | in / out  | `balance_transactions`                         | Gold standard. Easy. |
| **Braintree**           | in / out  | Settlement Batch Summary Report                | Daily SFTP + API |
| **PayPal**              | in / out  | TRR + SSR settlement reports                   | Webhooks lie; reports are truth |
| **Coinbase Commerce**   | in        | `charges` API + on‑chain confirmation          | Confirm finality on chain, not via Coinbase |
| **Coinbase Prime**      | in / out  | Transactions API + chain finality              | For treasury / vendor payouts in crypto |
| **Solana (USDC, SOL)**  | in / out  | `getSignaturesForAddress` @ `finalized`        | Never `confirmed` — reorgs happen |
| **Ethereum / Base**     | in / out  | Indexed via Helius/Alchemy @ N confirmations   | Per‑chain finality config |
| **Banks via Plaid**     | read      | `/transactions/sync` cursor                    | Good enough for SMB; breaks often |
| **Banks direct (ACH)**  | in / out  | NACHA returns + BAI2 statement files           | Enterprise tier; per‑bank build |
| **Wise**                | out       | Transfers API + statement                      | Multi‑currency vendor payouts |
| **Zelle**               | read‑only | Parsed out of bank transaction memos           | No third‑party Zelle API exists |
| **Venmo (business)**    | read‑only | Plaid item, fragile                            | Skip for v1 unless a customer demands |

For each row we run two loops: a **realtime ingestor** (webhooks + short‑poll
fallback) that posts within seconds, and a **scheduled reconciler** (every
1–15 min depending on rail) that pulls the provider's authoritative report
and proves zero drift.

---

## The killer UI: reconciliation breaks dashboard

This is the screen no incumbent does well, and the one a controller will
open every morning forever.

```
Reconciliation Breaks — last 24h                       Tenant: Acme Inc.

  Provider     Break type            Amount      Detected           Status
  ──────────   ───────────────────   ─────────   ────────────────   ────────
  Stripe       fee_mismatch          $   1.34    2026‑05‑17 09:14   open
  PayPal       missing_in_ledger     $  84.20    2026‑05‑17 08:02   open
  Chase ACH    duplicate_posting     $ 240.00    2026‑05‑17 03:55   auto‑resolved
  Solana       unknown_inbound       $1,500.00   2026‑05‑16 22:41   investigating
  Coinbase     fx_rate_drift         $   0.18    2026‑05‑16 18:30   acknowledged

  Total open break exposure: $1,585.72   (0.003% of 24h volume)
```

Every break has: which provider, what we expected, what we saw, the diff, the
ledger postings on our side, the provider's record on their side, and a
one‑click "auto‑resolve" if the rule applies (e.g. Stripe fee rounding under
$0.05). Nothing gets silently swallowed.

A clean ledger means *every break is either resolved or explicitly
acknowledged with a note.* That's the SOC 2 control. That's the audit trail.

---

## Why this is hard (and why we win by doing it anyway)

1. **Idempotency under provider chaos.** Stripe sometimes delivers the same
   webhook 4 times. PayPal sometimes delivers it never. ACH reversals show up
   60 days later. Every posting carries an `idempotency_key` and a
   `(source, source_event_id)` unique constraint. Replays are no‑ops.
2. **Eventual consistency, made explicit.** Every balance carries an
   `as_of_confidence` field — `pending`, `confirmed`, `finalized`. We don't
   pretend pending money is real, and we don't hide it either.
3. **On‑chain finality is non‑trivial.** Solana reorgs at `confirmed`,
   Ethereum reorgs at 1–2 blocks. We post `pending` at first sight, promote
   to `finalized` only at the chain's finality threshold, and revert with a
   compensating posting if a reorg eats the original tx.
4. **Tenant isolation is a day‑1 decision.** Shared schema + Postgres RLS for
   self‑serve tier, dedicated database per tenant for enterprise. Per‑tenant
   KMS data keys for provider secrets — no plaintext credentials ever leave
   the vault.
5. **Right‑to‑be‑forgotten vs append‑only ledger.** PII is tombstoned on
   request; financial postings are retained for the regulated period (7y US)
   with the PII redacted in place. Documented, audited.

---

## Platform primitives that come with the ledger

Three primitives ship in the same service as the ledger — not as a separate
product, because they all live on the same Postgres and inherit its HA story
"for free." All three become competitive differentiators once a tenant has
glued together what they actually need:

### 1. Tenant-scoped leases ("locks")

A B2B customer running a payment job, end-of-month close, or manual
adjustment can grab a lease on `customer:<id>` or `period:2026-05` while
they work, and release it when done. Backed by Postgres (HA == ledger HA),
TTL-based (no orphaned locks on a worker crash), opaque token (can't be
stolen by a third party that knows the resource key), and audited (every
acquire / renew / release / preempt / expire is a row).

```
POST   /v1/tenants/{t}/locks                          { resource, ttl_seconds, holder }
POST   /v1/tenants/{t}/locks/{resource}/renew         { lease_token, ttl_seconds }
DELETE /v1/tenants/{t}/locks/{resource}               { lease_token }
GET    /v1/tenants/{t}/locks
```

Why not Solana for locks: Solana's 12-20 second finality and per-tx cost
make it the wrong tool for any high-frequency coordination primitive.
Postgres advisory leases run sub-millisecond and inherit RDS multi-AZ
failover. (Solana stays valuable for anchoring the ledger postings as
tamper-evidence — different use case, different tool.)

### 2. Durable scheduler ("bulletproof cron")

A Postgres-backed job runner using the standard `FOR UPDATE SKIP LOCKED`
pattern (River / pg-boss / Sidekiq PG). Guarantees exactly-one execution
per due tick across N pods. Per-tenant schedules with IANA timezones.
Retries with exponential backoff. Failures after `max_attempts` land in
`dead_letter_jobs` for the breaks dashboard.

```
POST /v1/tenants/{t}/scheduled-jobs               { kind, name, schedule_kind, cron_expr|interval_seconds|one_shot_at,
                                                    timezone, payload, max_attempts, retry_backoff_secs, timeout_seconds }
GET  /v1/tenants/{t}/scheduled-jobs
GET  /v1/tenants/{t}/scheduled-jobs/{id}/runs    -> durable history (was it actually run? what happened?)
POST /v1/tenants/{t}/scheduled-jobs/{id}/run-now
POST /v1/tenants/{t}/scheduled-jobs/{id}/disable
POST /v1/tenants/{t}/scheduled-jobs/{id}/enable
```

Built-in job kinds shipped on day 1:

| Kind | Owner | What it does |
|---|---|---|
| `system.lock_sweeper` | platform | GC expired leases |
| `system.anchor_sweeper` | platform | Publish Merkle roots to Solana |
| `notifications.evaluate_rules` | platform | Walk active rules + emit dispatches |
| `tenant.webhook` | tenant | POST signed payload to tenant URL (the building block for tenant payroll / AP run schedules) |

Tenants don't run business logic on our platform; they register a
`tenant.webhook` job whose payload includes their own `webhook_url`, and on
schedule we POST a signed payload to *their* system. So "Friday 9am, run
payroll" becomes a scheduled `tenant.webhook` whose handler hits the
tenant's `/run-payroll` endpoint with an HMAC-signed body. Tenant's system
does the actual work; we provide the bulletproof trigger.

Why not Kubernetes CronJobs for this: K8s CronJobs are fine for a fixed
set of platform housekeeping, but they fall over the moment you need
per-tenant schedules, per-tenant timezones, or queryable run history.

### 3. Three-layer sync (and why we don't poll constantly)

Polling external providers on a fast cadence is expensive (API quota,
infrastructure cost, signal-to-noise on rate-limit errors) and almost
always the wrong default. The platform instead uses three layers, each
covering the others' failure modes:

| Layer | Cadence | Mechanism | Coverage |
|---|---|---|---|
| Webhooks | real-time | Provider pushes to `/v1/webhooks/{provider}` | New events, refunds, disputes, chargebacks — caught immediately |
| **On-demand sync** | **on user action** | `POST /v1/tenants/{t}/connections/{c}/sync` | "Refresh now" buttons; the dominant poll path |
| Backstop sync | **~5x/day** (default `interval_seconds: 18000`) | Scheduled `sync.connection` job per connection | Catches anything webhooks + on-demand missed (provider outage, dropped webhook, paused connection) |

All three feed the same `sync.connection` handler, which acquires a
tenant-scoped lease on `connection:<id>` so concurrent triggers don't
double-sync. The backstop schedule lives in `scheduled_jobs` like any
other job — tenants can adjust their own cadence, disable it entirely
(if they trust their webhooks), or trigger it manually via the
`run-now` endpoint.

```
POST /v1/tenants/{t}/connections/{c}/sync   { cursor?, trigger? }
       → 202 Accepted
       → { job: {...}, runs_url: ".../scheduled-jobs/<id>/runs?limit=1" }
```

The client polls `runs_url` (or subscribes to a future "job completed"
webhook) to see the sync result. The job typically completes within
seconds; large backfills paginate via `cursor`.

### 4. Notifications

Rules + dispatches, evaluated by the `notifications.evaluate_rules` job.
Built-in rule kinds:

- `balance_negative` — customer's AR account is credit-balanced (overpaid)
- `payment_overdue` — AR posting > `days` old still outstanding
- `payment_received` — credit posted to a clearing account
- `reconciliation_break_opened` — new row in `reconciliation_breaks`
- `lease_held_too_long` — a lease has been held > N minutes (operational)

Built-in channels: `webhook` (HMAC-signed POST), `slack` (incoming webhook),
`email` (SES/SendGrid driver — stub today), `sms` (Twilio driver — stub).

Per-`(rule, target_resource, day)` throttling so a customer doesn't get
emailed 100x as their balance oscillates around zero.

```
POST /v1/tenants/{t}/notification-rules            { kind, name, params, channel, target, throttle_per_day }
GET  /v1/tenants/{t}/notification-rules
GET  /v1/tenants/{t}/notification-dispatches       -> full delivery history (was it sent? did it bounce?)
```

## What we deliberately don't do (yet)

To stay shippable and out of regulatory minefields, v1 explicitly skips:

- **Money origination on our own license.** All payments are initiated via
  the customer's own connected accounts (Stripe Connect, PayPal Partner,
  their bank). We are a record‑keeping platform, not a money transmitter.
  This keeps us out of MSB licensing for the first 18+ months.
- **Multi‑currency FX engine.** We record FX postings from providers, we do
  not quote or hold FX positions.
- **Invoicing UI / customer portal.** API + webhooks only; customers' own
  systems own the invoice UX. Add later.
- **Tax (Avalara/Anrok territory).** Out of scope. We post tax as a separate
  ledger account; the calculation is the customer's.
- **AP origination on rails we don't trust yet.** Wire and ACH only via
  banks with real APIs. No crypto AP origination until OFAC screening
  pipeline is shipped.

---

## Concrete example: SaaS company, one customer, one vendor

**Customer "Acme Foo Inc." (cus_001), invoiced monthly:**

```
2026‑05‑01  invoice issued                AR/cus_001  +1999.00  revenue  -1999.00
2026‑05‑03  Stripe card payment received  clearing/stripe +1939.30  AR/cus_001 -1999.00
                                          fees/stripe    +59.70
2026‑05‑05  Stripe payout to Chase        cash/chase     +1939.30  clearing/stripe -1939.30
2026‑05‑10  customer disputes $200        AR/cus_001     +200.00   chargebacks    -200.00
2026‑05‑14  dispute lost                  fees/stripe    +15.00    cash/chase     -215.00
                                          chargebacks    +200.00
                                          AR/cus_001     -? (depends on policy)
```

`GET /v1/customers/cus_001/billing-state` now returns
`outstanding_balance_minor: 20000` (the $200 dispute) and a `next_bill`
queued for 2026‑06‑01.

**Vendor "Cloud Provider Bar" (ven_007), invoiced weekly:**

```
2026‑05‑10  bill received                 AP/ven_007  +3400.00  expenses      -3400.00
2026‑05‑13  bill approved                 (no ledger movement, state flip)
2026‑05‑15  ACH payment initiated         AP/ven_007  -3400.00  clearing/ach  +3400.00
2026‑05‑17  ACH settled                   clearing/ach -3400.00  cash/chase    -3400.00
```

`GET /v1/vendors/ven_007/payable-state` now returns
`outstanding_payable_minor: 0` and the next bill arrives on the regular
weekly cadence.

Both queries are O(1) reads off the per‑entity projection; the underlying
postings are the immutable proof.

---

## Why the wedge is defensible

Modern Treasury owns the bank‑rail story. Stripe owns cards. Plaid owns
read‑only bank data for SMBs. **Nobody owns the unified ledger across
processors + banks + on‑chain**, because each of those incumbents has a
reason not to build it (cannibalization, focus, regulatory).

Companies whose money lives in 5+ places — crypto‑native businesses,
marketplaces, international SaaS, creator platforms, treasury teams — are
stuck duct‑taping today. They are the design partners. They will pay.

The product is two API endpoints and one dashboard. The moat is the
reconciliation rules library we build over years, one provider quirk at a
time, plus the SOC 2 / compliance posture that makes us safe to plug into a
finance team's stack.

---

## Open questions for design partners

When we sit down with the first 10 prospects, these are the questions that
shape v1 scope:

1. Which 3 providers cause you the most reconciliation pain today?
2. How do you currently answer "when do I bill X / pay Y" — what system, how
   stale, how often is it wrong?
3. Do you move money on chain today? Which chains, which assets, what volume?
4. Self‑serve API or sales‑led with a CSM? (Pricing model follows.)
5. Self‑hosted ledger acceptable, or managed only?
6. Hard requirement: SOC 2 Type II, ISO 27001, or just SOC 2 Type I to start?
7. AP origination needed in v1, or is record‑keeping enough for year one?

The answers determine the order of the next 12 weeks. The two questions at
the top of this doc do not change.
