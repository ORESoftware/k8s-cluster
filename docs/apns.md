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
