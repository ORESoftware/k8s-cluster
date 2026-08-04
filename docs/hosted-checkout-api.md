# Quaestor hosted checkout API

`quaestor-checkout-api` is the service-to-service boundary for applications that
need to collect an advance payment without holding Stripe credentials or
creating payment state outside Quaestor.

The first consumer is Daedalus Fab. The API is intentionally narrow: it creates
and retrieves Stripe Checkout Sessions for an allow-listed Quaestor tenant. It
does not expose the general tenant API and it does not accept end-user JWTs.

## Trust boundary

- The caller authenticates with `BILLING_CHECKOUT_API_BEARER`.
- The service only serves tenants in `BILLING_CHECKOUT_ALLOWED_TENANT_IDS`.
- The selected provider connection must be an active Stripe connection with an
  `acct_...` external account identifier.
- If a tenant has more than one active Stripe connection, exactly one must have
  `metadata.checkout_default=true`.
- The caller supplies `success_url` and `cancel_url`, but both must be under an
  origin/path prefix in `BILLING_CHECKOUT_RETURN_URL_PREFIXES`.
- Hosted redirect URLs returned by Stripe must use HTTPS and a host in
  `BILLING_CHECKOUT_ALLOWED_HOSTS` (default: `checkout.stripe.com`).
- Customer email is sent to Stripe for Checkout but is not stored in plaintext
  by Quaestor. Only a domain-separated SHA-256 correlation hash is retained.
- Success/cancel URLs are fingerprinted as part of the idempotent intent, but
  are not persisted.

The public Daedalus web server never receives the Quaestor bearer or Stripe
secret. Daedalus API calls this internal service over the cluster network.

## Schema

The declarative schema fragment is:

```text
schema/fragments/050_checkout_sessions.sql
```

Review and apply it with the repository's normal DPM workflow:

```bash
SHADOW_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres \
TARGET_DATABASE_URL="$BILLING_DATABASE_URL" \
  scripts/dpm.sh verify

SHADOW_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres \
TARGET_DATABASE_URL="$BILLING_DATABASE_URL" \
  scripts/dpm.sh diff
```

Apply only after reviewing the generated SQL.

## Run

```bash
cargo run --bin quaestor-checkout-api
```

Required environment:

| Variable | Purpose |
| --- | --- |
| `BILLING_DATABASE_URL` (or `DATABASE_URL`) | Quaestor Postgres database |
| `BILLING_CHECKOUT_API_BEARER` | Dedicated random service credential, at least 32 bytes |
| `BILLING_CHECKOUT_ALLOWED_TENANT_IDS` | Comma-separated Quaestor tenant UUID allow-list |
| `STRIPE_API_KEY` (or `STRIPE_CLIENT_SECRET`) | Stripe platform credential used with `Stripe-Account` |
| `BILLING_CHECKOUT_RETURN_URL_PREFIXES` | Comma-separated approved HTTPS origin/path prefixes |

Optional environment:

| Variable | Default | Purpose |
| --- | --- | --- |
| `BILLING_CHECKOUT_HOST` | `0.0.0.0` | Listen address |
| `BILLING_CHECKOUT_PORT` | `8088` | Listen port |
| `STRIPE_API_VERSION` | repository Stripe API default | Explicit provider version |
| `BILLING_STRIPE_API_BASE` | `https://api.stripe.com` | Override for local provider fixtures only |
| `BILLING_CHECKOUT_ALLOWED_HOSTS` | `checkout.stripe.com` | Allowed hosted-checkout redirect hosts |

The API should be reachable only from trusted workloads. Do not publish it
through a public ingress.

## Create a checkout

```http
POST /internal/v1/tenants/{tenant_id}/checkout-sessions
Authorization: Bearer <service credential>
Idempotency-Key: daedalus:<plan-id>:<quote-fingerprint>:r1
Content-Type: application/json
```

```json
{
  "client_reference_id": "01900000-0000-7000-8000-000000000001",
  "amount_minor": 12500,
  "currency": "USD",
  "description": "CNC motorcycle instrument bracket deposit",
  "customer_email": "rider@example.com",
  "success_url": "https://fab.example/jobs/dpt_.../success?session_id={CHECKOUT_SESSION_ID}",
  "cancel_url": "https://fab.example/jobs/dpt_.../cancel",
  "metadata": {
    "application": "daedalus-fab",
    "vehicle_kind": "motorcycle",
    "part_category": "mounting_bracket"
  }
}
```

Response (`201 Created` for a new intent, `200 OK` for a replay):

```json
{
  "id": "cs_test_...",
  "url": "https://checkout.stripe.com/c/pay/...",
  "status": "open",
  "payment_status": "unpaid",
  "amount_minor": 12500,
  "currency": "USD",
  "client_reference_id": "01900000-0000-7000-8000-000000000001",
  "provider_connection_id": "...",
  "quaestor_id": "...",
  "created_at": "2026-08-04T20:00:00Z",
  "updated_at": "2026-08-04T20:00:00Z"
}
```

Reusing an `Idempotency-Key` with any changed intent field returns `409
Conflict`. The service persists the intent before calling Stripe, then calls
Stripe with a deterministic provider idempotency key. A retry after a process or
network failure therefore recovers the original Checkout Session rather than
creating another payment.

## Retrieve and reconcile

```http
GET /internal/v1/tenants/{tenant_id}/checkout-sessions/{cs_id}
Authorization: Bearer <service credential>
```

The service retrieves the session from Stripe on the same connected account,
validates its amount, currency, client reference, state, and hosted URL, then
persists and returns the refreshed state.

Quaestor's main server continues to verify and retain Stripe webhook events in
`webhook_events`. The retrieval endpoint gives application workflows a direct,
idempotent reconciliation path even when webhook delivery or downstream event
processing is delayed.

## Stripe metadata

Quaestor adds these protected keys to both the Checkout Session and its
PaymentIntent:

- `quaestor_checkout_id`
- `quaestor_tenant_id`
- `quaestor_client_reference_id`

Caller metadata is copied to both objects after validation. Callers cannot use
the `quaestor_` prefix.

## Operational checks

- `/health` is process liveness.
- `/ready` verifies database connectivity.
- Logs never include bearer credentials, Stripe API keys, request bodies,
  customer email, or return URLs.
- Stripe request IDs and bounded provider error details are logged for support.
- Rotate the dedicated bearer independently from human tenant authentication.
