# Expo Push adapter

The Expo adapter implements the complete Expo push-ticket and push-receipt lifecycle over the shared `PushJob` v1 contract.

## Configuration

`ExpoConfig::new` accepts an optional project access token. Expo Push can operate without this token when enhanced push security is disabled for the project. When configured, the token is sent only as an HTTPS bearer credential and must come from a server-side secret.

Public configuration uses only Expo's fixed HTTPS endpoints without embedded credentials:

- send: `https://exp.host/--/api/v2/push/send`
- receipts: `https://exp.host/--/api/v2/push/getReceipts`

Loopback/custom endpoints exist only in test-only constructors.

## Push tickets

- A send request contains between 1 and 100 messages.
- One batch may contain jobs for only one application/project.
- Expo push tokens must use the `ExpoPushToken[...]` or legacy `ExponentPushToken[...]` form.
- Optional title, body, custom data, image, TTL, priority, and collapse ID values are mapped into the Expo request.
- Expo can return HTTP 200 while individual tickets contain errors. Every ticket is parsed and normalized independently.
- Accepted ticket IDs are retained as provider metadata so the receipt worker can perform follow-up processing.

## Push receipts

- A receipt lookup contains between 1 and 1,000 ticket IDs.
- Missing receipts are treated as retryable because Expo may not have produced the final receipt yet.
- Receipt-level `DeviceNotRegistered` disables the installation/token through the downstream registry lifecycle.
- `MessageTooBig`, `MessageRateExceeded`, `MismatchSenderId`, and `InvalidCredentials` are normalized into invalid-payload, throttled, or permanent-provider outcomes.

Ticket IDs are operational correlation values, but complete Expo device tokens and access tokens remain capability secrets and must not appear in logs or result events. Normalized outcomes use the target fingerprint.

## Retry behavior

HTTP throttling and server failures use the shared status classifier and honor `Retry-After` when present. Request-level Expo errors are classified separately from per-ticket and per-receipt errors.

## Tests

The adapter test suite covers:

- token-shape validation
- message mapping
- optional bearer authorization
- multi-message send batches
- HTTP-200 per-ticket errors
- receipt request batches and receipt-level outcomes
- missing receipt behavior
- request-level throttling/permanent errors
- bounded safe details and target fingerprinting

No real Expo access token or device token is required for tests. Mock endpoints are loopback-only and inaccessible through the public configuration API.

The merge gate requires locked formatting, Clippy with warnings denied, all tests, the Rust 1.88 container build, cargo-deny, RustSec, and full-history Gitleaks to pass on the same reviewed commit. Compiler-guided fixes remain isolated to the Expo adapter and are rerun through the full permanent matrix before merge.
