# Communications Postgres contract

This directory defines the durable state shared by push, email, SMS, and postal delivery workers. It does **not** turn Postgres or Supabase into a delivery provider. Provider credentials remain in workload identity or Kubernetes External Secrets and are never stored in these tables.

## Files

- `schema.sql` — portable Postgres 17 contract for endpoints, preferences, jobs, attempts, provider webhook requests, append-only receipts, suppressions, and the transactional outbox.
- `supabase.sql` — Supabase/PostgREST RLS and redacted owner projections. Apply after `schema.sql`.
- `supabase-rls.test.sql` — adversarial owner-isolation test used in CI.

`schema.sql` is a declarative dpm source. Never apply it directly to production and never migrate at service startup. The owning deployment must expose a reviewed `scripts/dpm.sh {diff|verify|review|apply}` workflow with `--schemas communications`.

## Service split

The push worker remains independently deployable and owns FCM, APNs, Expo, and Web Push. Email, SMS, and postal adapters use the same communications ledger but are separate delivery lanes. A channel-neutral orchestrator chooses channels from verified preferences and records each provider attempt.

This avoids recreating the old `dd-email-sms-contact-rs` monolith while still allowing policies such as:

1. Try push.
2. If push is unavailable or the policy requires escalation, try SendGrid email.
3. Use Twilio SMS only after verified opt-in and suppression checks.
4. Use a postal provider only for explicitly approved purposes and verified addresses.

## Sensitive data

The schema stores recipient capabilities and message content only as application-encrypted ciphertext plus a domain-separated fingerprint. This includes:

- push tokens and Web Push subscriptions
- email addresses
- phone numbers
- postal addresses
- rendered message bodies and template variables

Provider webhook bodies are not persisted. `webhook_requests` and `receipts` retain SHA-256 digests, signature-verification results, provider correlation identifiers, normalized status, and sanitized metadata.

SendGrid `custom_args` and Twilio callback URLs must carry only opaque job/attempt IDs. They must not contain email addresses, phone numbers, names, message text, JWTs, or tenant secrets.

## Receipts and reconciliation

Provider events are append-only and may arrive out of order. The service must retain every deduplicated event and project current state using provider-specific transition rules rather than `received_at` order alone.

- SendGrid: dedupe by `sg_event_id`; correlate opaque attempt IDs from custom arguments; verify the signed Event Webhook against the exact timestamp plus raw body before JSON parsing.
- Twilio: correlate by Message SID; verify `X-Twilio-Signature` against the externally visible callback URL and the exact form/body; derive a deterministic event ID when Twilio does not supply one; reconcile messages that remain non-terminal.
- Push: store normalized outcomes and provider receipt IDs without storing full target capabilities.
- Postal: correlate provider piece IDs and append production/tracking/return events.

Invalid push tokens, hard email bounces, SMS STOP/undeliverable responses, and returned postal addresses create durable suppressions rather than blind retries.

## Authentication

User-facing APIs authenticate through `github.com/shared-auth` with Supabase as a supported authority. The verified identity must provide a stable `shared_user_id`; Supabase users are additionally linked by `supabase_user_id`.

RLS uses only verified JWT claims:

- `sub` for the Supabase user UUID
- `shared_user_id` for the stable cross-provider identity

User-editable metadata is never an authorization source. Plaintext endpoint registration goes through the authenticated service, which validates and encrypts the target before insertion. Direct clients may manage only their own preferences and query sanitized endpoint/history views.

Provider webhooks do not use user JWTs. They fail closed on provider signature verification and freshness/replay checks.

## Claiming work

Workers claim `communications.jobs` and `communications.outbox` rows inside a transaction with `FOR UPDATE SKIP LOCKED`, set a bounded lease, commit, and perform network I/O outside the transaction. A crashed worker may cause at-least-once execution, so deterministic idempotency keys and provider correlation IDs remain mandatory.
