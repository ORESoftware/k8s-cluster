# Provider and app-store interoperability

This guide explains how mobile applications, app stores, push providers, email, SMS, Supabase/Postgres, and `shared-auth` fit together around `push-notification-server.rs`.

## System boundary

Google Play and Apple App Store are distribution, signing, entitlement, and release systems. They do not deliver push notifications.

| Concern | System |
|---|---|
| Android application distribution | Google Play Console |
| iOS application distribution | App Store Connect / TestFlight |
| Android push delivery | Firebase Cloud Messaging HTTP v1 |
| Native Apple push delivery | Apple Push Notification service |
| Expo-managed push delivery | Expo Push Service, backed by FCM/APNs credentials |
| Transactional email | SendGrid Mail Send |
| Transactional SMS | Twilio Messages |
| Endpoint, preference, job, attempt, receipt, suppression, outbox state | shared Postgres communications schema |
| User/service identity | `shared-auth` plus Supabase token verification |

The server keeps push and contact contracts separate:

- `PushJob` / `PushOutcome`: FCM, APNs, Expo, and Web Push.
- `ContactJob` / `ContactOutcome`: SendGrid email and Twilio SMS.
- Provider acceptance is not final delivery. Final email/SMS state comes from signed provider callbacks and durable receipts.

## Provider-selection rules

| Client path | Recommended provider | Notes |
|---|---|---|
| Native Android | FCM HTTP v1 | Direct control, Firebase registration token required. |
| Native iOS | APNs | Direct control, bundle topic and environment must match. |
| Expo managed workflow | Expo Push | Simplest cross-platform route when EAS credentials are managed correctly. |
| Expo app needing direct provider control | FCM/APNs | Store native device tokens instead of Expo tokens and route explicitly. |
| Browser | Web Push | Uses VAPID and browser push-service endpoints. |
| Push unavailable or user opted into email | SendGrid | Separate contact job and durable callback receipts. |
| Time-sensitive SMS fallback | Twilio | Requires consent, E.164 number, sender configuration, and status callbacks. |

Do not send the same business event through several channels independently. Create one communication intent and use deterministic idempotency keys plus a channel policy so fallback does not become duplicate delivery.

## Shared prerequisites

Before enabling any provider:

1. Define tenant and application identifiers.
2. Configure authenticated HTTP or ACL-protected NATS producers.
3. Register endpoints through an authenticated service that validates and encrypts capability-bearing values before Postgres insert.
4. Persist communication intent and idempotency state in the shared communications schema.
5. Keep provider credentials in workload identity or Kubernetes External Secrets.
6. Enable target redaction, structured outcomes, metrics, and provider-specific canaries.
7. Separate provider acceptance from final receipt processing.

Never log or place in dead letters:

- device tokens;
- Expo push tokens;
- APNs topics plus tokens as a reconstructable pair;
- Web Push endpoints or subscription keys;
- email addresses or phone numbers;
- message bodies;
- provider credentials, JWTs, or callback secrets.

## Android, Firebase, and Google Play

### Identity alignment

The following values must describe the same Android application:

- Android package/application ID in the mobile build;
- Firebase Android app package name;
- Google Play Console application package name;
- application identifier used in the communications ledger;
- environment represented by the endpoint registration.

A Play release does not automatically configure FCM. The installed app obtains an FCM registration token through the Firebase SDK, then sends that token to the authenticated registration API.

### Server credentials

Use FCM HTTP v1 with a service account or workload identity that has only the required messaging permission. Configure the server-side Firebase project identity and credential source. Never ship the server credential in an Android bundle.

### Client lifecycle

The Android client must:

1. initialize Firebase for the correct package and environment;
2. request notification permission on Android versions that require runtime permission;
3. obtain the current registration token;
4. register the token with tenant, application, installation, environment, locale, timezone, and preference metadata;
5. upload token rotations immediately;
6. revoke the endpoint on logout, account removal, or device removal;
7. handle foreground, background, and notification-tap behavior separately.

A token belongs to an installation, not permanently to a user. Account changes must update ownership without leaking the previous user's endpoint.

### Google Play release matrix

Test at least:

| Track | Build signing | FCM project | Expected use |
|---|---|---|---|
| Local/debug | debug signing | non-production Firebase project | developer testing |
| Internal testing | Play-managed release signing | staging or production-like project | integration testing |
| Closed/open testing | release signing | production candidate project | rollout validation |
| Production | release signing | production Firebase project | live delivery |

Validate that the final Play-distributed artifact uses the intended Firebase configuration. Package-name equality alone does not prove that the embedded Firebase project is correct.

### FCM failure handling

- Treat invalid/unregistered tokens as permanent endpoint failures and suppress or revoke them while retaining audit history.
- Honor throttling and `Retry-After` as retryable outcomes.
- Bound retries for transient provider failures.
- Do not blindly retry malformed payloads or cross-project token mismatches.

## Apple, APNs, TestFlight, and App Store

### Identity alignment

The following must agree:

- Apple Developer App ID / bundle identifier;
- application bundle identifier in the binary;
- APNs topic used by the server;
- App Store Connect application record;
- provisioning profile and push entitlement;
- communications-ledger application and environment.

The App Store distributes the app. APNs delivers notifications to the device token obtained by the installed app.

### APNs credentials

For token authentication, configure server-side:

- Apple team ID;
- APNs key ID;
- P-256 private key;
- bundle topic;
- explicit production or sandbox environment.

Keep the private key outside the repository and mobile binary. Rotate keys deliberately and support overlap where operationally possible.

### Environment rules

Development builds generally receive sandbox APNs tokens. TestFlight and App Store builds use production APNs. Tokens are environment-specific and must never be sent to the wrong APNs host.

Store environment alongside each endpoint. Do not infer environment only from build labels at send time.

### Client lifecycle

The Apple client must:

1. include the push notification entitlement;
2. request user authorization where appropriate;
3. register for remote notifications;
4. upload the returned device token to the authenticated registration API;
5. replace rotated tokens rather than creating uncontrolled duplicates;
6. revoke ownership on logout/device removal;
7. handle foreground presentation, background delivery constraints, and notification responses.

### TestFlight/App Store matrix

- Verify the exact bundle ID and entitlements in the archived binary.
- Test direct APNs delivery against a TestFlight-installed build before production rollout.
- Confirm production APNs host selection.
- Validate foreground/background behavior and notification categories.
- Confirm permanent APNs failures update endpoint health without deleting receipt history.

Common causes of APNs failure include topic mismatch, wrong environment, expired/revoked key, malformed token, missing entitlement, and an app build signed for a different identifier.

## Expo interoperability

Expo Push is a distinct provider path. The mobile app obtains an Expo push token, while Expo uses the configured Android and Apple credentials to reach FCM and APNs.

### Project identity

Keep these aligned:

- Expo project identity/project ID;
- Android package name;
- iOS bundle identifier;
- EAS build profile/environment;
- Firebase and Apple credentials configured for that project;
- application/environment stored with the Expo endpoint.

### Choosing Expo versus direct delivery

Use Expo Push when the product accepts Expo-managed routing and receipt semantics. Use direct FCM/APNs when you need native-provider features, tighter provider isolation, direct credential ownership, or a migration away from Expo.

Do not treat Expo tokens and native device tokens as interchangeable. Store their provider type explicitly.

### Expo lifecycle and receipts

- Validate Expo token shape before enqueueing.
- Batch requests within provider limits.
- Persist ticket identifiers from accepted requests.
- Poll or process Expo receipts separately.
- Mark `DeviceNotRegistered` and equivalent permanent results as endpoint-invalidating outcomes.
- Bound receipt polling and retain the original attempt correlation.

### Migration

During a controlled migration between Expo and direct providers:

1. allow an installation to register both endpoint types with explicit provider/environment fields;
2. choose one canonical route per communication intent;
3. shadow outcomes without delivering duplicates;
4. compare acceptance and final invalid-token behavior;
5. retire the old endpoint only after measured cutover.

## SendGrid interoperability

### Configuration

Configure server-side:

- least-privilege Mail Send API key;
- verified sender email and optional name;
- global or EU API region;
- optional sandbox mode;
- controlled dynamic template IDs where templates are used.

The producer supplies content or an approved template reference through `ContactJob`; it cannot supply credentials or override the server-owned sender.

### Acceptance and receipts

A successful Mail Send response means SendGrid accepted the request. It does not prove delivery.

Use opaque attempt correlation in controlled `custom_args`. Final state requires signed Event Webhook processing that:

- verifies the exact timestamp and raw body before parsing;
- rejects stale or replayed requests;
- deduplicates by provider event identity;
- maps delivered, deferred, bounce, dropped, spam report, and unsubscribe states to normalized receipts;
- creates suppressions for permanent or consent-related outcomes;
- never logs recipient addresses or bodies.

### Testing

- Use sandbox mode to validate request construction without sending mail.
- Run the secret-gated provider canary only with dedicated test recipients and a verified sender.
- Test explicit text/HTML mode and dynamic-template mode separately.
- Confirm retries honor throttling and do not duplicate accepted mail.

## Twilio interoperability

### Configuration

Prefer API Key credentials for production rotation. Configure exactly one sender mode:

- Messaging Service SID; or
- approved E.164 `From` number.

Also configure an HTTPS status callback URL, validity period, and tenant/provider rate limits. The producer cannot choose credentials or sender identity.

### Consent and number handling

- Normalize and validate recipient numbers as E.164.
- Record consent and purpose in the shared communications schema.
- Enforce STOP/opt-out and suppression state before sending.
- Never expose full numbers in outcomes, logs, metrics, or dead letters.

### Acceptance and receipts

A successful Messages API response means Twilio accepted the request. Persist the Message SID as provider correlation data.

Final status callback processing must:

- verify `X-Twilio-Signature` against the externally visible URL and complete request parameters;
- support JSON-body hash verification where applicable;
- reject missing, invalid, stale, or replayed callbacks;
- normalize queued, sent, delivered, undelivered, failed, and canceled states;
- reconcile non-terminal messages with bounded polling where needed;
- create endpoint-health or suppression updates for STOP, invalid, or repeatedly undelivered numbers.

### Testing

- Use Twilio test credentials for request-path validation where supported.
- Use dedicated test numbers and Messaging Services for live canaries.
- Test sender selection, E.164 validation, 1,600-character ceiling, validity period, throttling, and callback verification.

## Authentication and authorization

User-facing registration, preference, and history APIs must validate Supabase access tokens through the canonical `shared-auth` contract. Authorization must use verified claims such as `sub`, `shared_user_id`, tenant, and application scope. User-editable metadata is not authoritative.

Internal outbox publishers and workers use service identities or tightly scoped NATS permissions. Provider webhooks do not use user JWTs; they use provider-specific signature verification.

Production must fail closed when configured authentication cannot be validated. A shared secret may remain only as an explicitly bounded migration mechanism, not as the canonical identity model.

## Postgres communication history

The shared communications schema should allow an operator to explain:

`business intent -> communication job -> provider attempt -> immediate acceptance -> final receipt -> suppression or endpoint-health change`

Use deterministic idempotency keys based on business event, tenant, application, recipient endpoint, purpose, channel, and contract version. Provider retries and callback redelivery must not create uncontrolled duplicate communications or receipts.

## HTTP examples

Push submission:

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $SERVER_AUTH_SECRET" \
  -H "Content-Type: application/json" \
  --data @push-job.json \
  http://localhost:8121/v1/push/jobs
```

Contact submission:

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $SERVER_AUTH_SECRET" \
  -H "Content-Type: application/json" \
  --data @contact-job.json \
  http://localhost:8121/v1/contact/jobs
```

Use synthetic values in fixtures. Never commit real recipient capabilities.

## Environment checklist

| Area | Required checks |
|---|---|
| Identity | tenant/application mapping, `shared-auth`, Supabase issuer/audience/JWKS |
| Android | package ID, Firebase project, FCM credential, Play-distributed config |
| Apple | bundle ID, topic, team/key IDs, APNs environment, entitlement |
| Expo | Expo project ID, EAS profile, FCM/APNs credentials, token type |
| SendGrid | verified sender/domain, API region, API key, sandbox/canary |
| Twilio | credential mode, Messaging Service/From, callback URL, consent |
| Database | endpoint encryption, preferences, idempotency, attempts, receipts, suppressions |
| Operations | rate limits, retries, DLQ, metrics, dashboards, alerts, canaries |

## Test strategy

Run permanent local/CI tests:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

The repository also exercises real-socket HTTP tests, live JetStream compatibility, container builds, dependency policy, RustSec, and Gitleaks. Provider canaries are secret-gated and must skip safely when credentials are absent.

Before a production rollout, test:

- one Android build installed from the intended Play track;
- one iOS build installed from TestFlight;
- Expo ticket plus receipt processing when Expo is enabled;
- SendGrid sandbox and signed Event Webhook flow;
- Twilio test/live canary plus signed status callback flow;
- duplicate job, provider timeout ambiguity, callback replay, database outage, and pod termination;
- endpoint revocation and account/device ownership changes.

## Troubleshooting map

| Symptom | Likely area |
|---|---|
| FCM sender mismatch or invalid token | wrong Firebase project, stale token, package/config mismatch |
| APNs bad device token | wrong environment, malformed/stale token, topic mismatch |
| APNs topic error | bundle ID/topic/credential mismatch |
| Expo ticket accepted but later fails | inspect Expo receipt and endpoint lifecycle |
| Email accepted but not delivered | inspect signed SendGrid events and suppressions |
| SMS accepted but not delivered | inspect Twilio callback state/error code and consent/sender configuration |
| Works locally but not store build | embedded project config, signing, entitlement, or environment mismatch |
| Duplicate user messages | missing deterministic idempotency or independent fallback producers |
| Cross-tenant target risk | missing verified tenant/application binding or overly broad NATS/API authorization |

## Operational ownership

- Push adapter correctness: FCM/APNs/Expo/Web Push implementation issues.
- Email/SMS acceptance: SendGrid/Twilio delivery lane.
- Final email/SMS state: signed callback reconciliation.
- Identity/authorization: `shared-auth` communications contract.
- Durable state: shared Postgres communications definitions.
- Distribution/signing: each product's Play Console, Apple Developer, App Store Connect, and EAS release processes.
