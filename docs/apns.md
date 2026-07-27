# APNs provider adapter

The APNs adapter implements token-authenticated device push over the shared `PushJob` v1 contract.

## Configuration boundary

Create one provider instance per Apple environment:

- production → `api.push.apple.com`
- sandbox/development → `api.sandbox.push.apple.com`

Each instance owns one key ID, Team ID, bundle/topic, `.p8` signing key, HTTP/2 client, and provider-token cache. A production provider rejects sandbox targets and a sandbox provider rejects production targets.

## Provider-token lifecycle

The adapter signs ES256 JWTs with:

- header: `alg=ES256`, `kid=<Apple key ID>`
- claims: `iss=<Apple Team ID>`, `iat=<current Unix time>`

A single refresh lock prevents concurrent callers from creating multiple provider tokens. Tokens are reused for 50 minutes, fitting Apple's documented requirement to avoid regenerating more frequently than every 20 minutes while replacing tokens before they become one hour old.

## Notification requests

Requests use `/3/device/<hex device token>` and include:

- `authorization: bearer <provider token>`
- `apns-topic`
- `apns-push-type`
- `apns-priority`
- `apns-expiration`
- optional `apns-collapse-id`
- optional `apns-id` when the idempotency key is a canonical lowercase UUID

Alert jobs use `apns-push-type=alert`. Data-only jobs use `background`, priority `5`, and `content-available=1`.

## Payload safety

- Application data is placed beside the top-level `aps` dictionary.
- Producers may not provide a custom `aps` key.
- Image notifications require a title or body and set `mutable-content=1`.
- Serialized payloads are limited to 4096 bytes.
- Complete device tokens and provider tokens never appear in normalized results or routine logs.

## Result classification

Apple response reasons are normalized into:

- invalid token: `BadDeviceToken`, `DeviceTokenNotForTopic`, `Unregistered`
- invalid payload: malformed headers, path, topic, expiration, priority, collapse ID, or payload
- throttled: `TooManyProviderTokenUpdates`, `TooManyRequests`
- transient: `IdleTimeout`, `InternalServerError`, `ServiceUnavailable`, `Shutdown`
- permanent provider/configuration failure: invalid, missing, or expired provider tokens; certificates; forbidden topics

The Apple request ID or reason code may be retained as `provider_code`; target capability data is represented only by its fingerprint.

## Test coverage

The adapter's test suite covers:

- key ID and Team ID validation
- production and sandbox endpoint selection
- ES256 JWT header and claim construction
- provider-token cache reuse
- alert and data-only payload construction
- reserved `aps` rejection and payload-size enforcement
- Apple reason-code classification
- target-environment mismatch rejection
- a mock APNs endpoint that verifies authorization, topic, push type, request ID, device-token path, and JSON payload

No real Apple key or device token is required for these tests. The mock delivery test seeds a synthetic cached provider token and uses a loopback HTTP endpoint that is inaccessible through public configuration APIs.

The merge gate requires locked formatting, Clippy with warnings denied, all tests, the Rust 1.88 container build, cargo-deny, RustSec, and full-history Gitleaks to pass on the same reviewed commit.

## Delivery record

PR #8 contains the implementation. The reviewed green source SHA and merge commit are recorded in Linear after the permanent gate completes.
