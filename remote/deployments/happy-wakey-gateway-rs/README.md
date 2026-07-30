# Happy Wakey gateway

`happy-wakey-gateway-rs` is the narrow public backend for the native
`ORESoftware/happy-wakey.rs` desktop app.

The desktop exchanges its Supabase access token at canonical shared-auth, then
presents the resulting short-lived shared-auth access token to this service.
The gateway introspects that token with a backend-only service credential. It
never accepts a NATS subject, contact-service endpoint, email destination, or
cluster credential from the desktop.

## First vertical slices

- `GET /v1/bootstrap` returns bounded capability and reminder-count data.
- `PUT /v1/reminders/sync` reconciles the signed-in user's pending email
  reminders.
- `DELETE /v1/reminders/jobs/:job_id` cancels one pending reminder.
- `POST /v1/reminders/test` queues a test reminder.
- A single scheduler process persists jobs as an atomic, mode-`0600` JSON file
  on a Kubernetes PVC using the cross-cloud `dd-block` storage class and sends
  due email requests to the fixed
  `dd.remote.contact.email.send` lane.
- `dd-email-sms-contact-rs` owns SendGrid. Twilio, push, geolocation, MCP, and
  task-manager capabilities remain disabled until a connector with an
  appropriate verification/consent contract is configured.

This first queue is intentionally single-replica. It survives process and pod
restarts, retries failed or timed-out NATS request/reply deliveries with bounded
backoff, and deduplicates calendar reconciliation by job/idempotency key. The
contact worker echoes the idempotency key and reports the SendGrid outcome
before the gateway records a job as dispatched. Core NATS can still lose an
in-flight request during a crash; JetStream persistence and contact-worker
idempotency remain follow-up work.

## Configuration

| Variable | Required | Purpose |
| --- | --- | --- |
| `SHARED_AUTH_BASE_URL` | yes | In-cluster shared-auth base URL |
| `SHARED_AUTH_INTROSPECT_SECRET` | yes | Service credential for `/auth/introspect` |
| `NATS_URL` | yes | In-cluster NATS endpoint |
| `NATS_SHARED_SECRET` | optional | Per-message contact-lane credential |
| `HAPPY_WAKEY_STATE_PATH` | no | Persistent JSON path; defaults to `/var/lib/happy-wakey/reminders.json` |
| `HAPPY_WAKEY_REMINDER_HORIZON_SECONDS` | no | Accepted future horizon; defaults to 14 days |
| `HAPPY_WAKEY_SCHEDULER_INTERVAL_SECONDS` | no | Due-job scan interval; defaults to 15 seconds |
| `HAPPY_WAKEY_SMS_ENABLED` | no | Capability flag only; SMS submission remains disabled |
| `HAPPY_WAKEY_PUSH_ENABLED` | no | Capability flag only; push submission remains disabled |
| `GEOLOCATION_BASE_URL` | no | Enables geolocation capability discovery only |
| `MCP_BROKER_BASE_URL` | no | Enables MCP capability discovery only |
| `TASK_MANAGER_BASE_URL` | no | Enables task-manager capability discovery only |

## Local checks

```bash
cargo fmt --check
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo run --locked -- --export-openapi > /tmp/happy-wakey-openapi.json
```

The service exposes `/healthz`, `/readyz`, `/metrics`, `/api/docs.json`,
`/docs/api`, and `/api/docs`. Metrics and logs contain counts and stable error
codes only, never JWTs, email addresses, message bodies, or upstream errors.
