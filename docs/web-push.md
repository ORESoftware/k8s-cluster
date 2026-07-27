# Web Push/VAPID adapter

The Web Push adapter encrypts browser push payloads, signs requests with VAPID, and treats every subscription endpoint as attacker-influenced capability data.

## Configuration

A provider requires:

- a P-256 VAPID private key in PKCS#8 or SEC1 PEM form
- a VAPID subject using `mailto:` or HTTPS
- a default TTL, normally 43,200 seconds
- a host policy

The private key is parsed during configuration and belongs in an external secret. Complete subscription endpoints, `p256dh` values, authentication secrets, and VAPID key material must never appear in logs or result events.

## Cryptographic boundary

The adapter uses Mozilla ECE directly for RFC 8291 `aes128gcm` payload encryption. It builds the VAPID authorization token with the service's ES256 JWT implementation and derives the uncompressed public VAPID key from the configured P-256 private key.

This deliberately avoids an unrelated RSA implementation. Web Push VAPID requires ES256; the dependency graph must not retain an unused RSA path with an unresolved security advisory. The final locked dependency graph is verified by RustSec and cargo-deny in CI.

## Default endpoint policy

The default policy permits only exact hosts or real subdomains of these browser push-service suffixes:

- `fcm.googleapis.com`
- `push.services.mozilla.com`
- `notify.windows.com`
- `push.apple.com`

Suffix matching is boundary-aware. A hostname such as `push.services.mozilla.com.attacker.invalid` or `evilpush.services.mozilla.com` does not match.

Every endpoint must:

- use HTTPS
- use port 443
- contain no embedded username or password
- contain no fragment
- include a host
- not use localhost
- not use a blocked IP literal

The HTTP client disables redirects. This prevents an allowlisted push endpoint from redirecting the request to an internal service.

## Blocked addresses

The adapter rejects IPv4 and IPv6 destinations that are loopback, private, link-local, unspecified, multicast, documentation-only, broadcast, CGNAT, benchmarking, reserved, unique-local, site-local, or IPv4-mapped forms of blocked addresses.

## Opt-in any-public-host mode

`WebPushHostPolicy::AnyPublic` is intentionally weaker and must be explicitly selected. Before delivery it resolves the hostname and rejects the request when any returned address is non-public or when resolution returns no addresses.

DNS validation is defense in depth, not a complete DNS-rebinding solution, because the HTTP client's later connection can perform another resolution. Production deployments should prefer the strict allowlist and restrict pod egress with Kubernetes NetworkPolicy or an egress proxy.

## Payload and delivery behavior

- Subscription `p256dh` and `auth` values must be unpadded URL-safe base64 and decode to the required P-256 public-key and 16-byte authentication-secret lengths.
- Payloads use the `aes128gcm` Web Push content encoding.
- Plaintext JSON is limited to 3,072 bytes to leave room for encryption overhead.
- VAPID claims contain the endpoint origin as `aud`, the configured subject as `sub`, and a bounded expiration.
- TTL is taken from `PushJob` or the configured default.
- Normal/high priority maps to the Web Push `Urgency` header.
- Collapse keys are SHA-256-derived into a fixed 32-character `Topic` header without exposing the original value.
- Provider redirects are never followed.

## Result classification

- HTTP 404 or 410: invalid/expired subscription
- HTTP 413 or 400: invalid payload
- HTTP 429: throttled
- HTTP 5xx and retryable status codes: transient provider failure
- HTTP 401 or 403: permanent provider/VAPID rejection

`Retry-After` is preserved when present. Transport errors use generic bounded descriptions so reqwest cannot leak a capability-bearing endpoint URL through its error text.

## Redaction

`redact_web_push_endpoint` retains only the scheme, host, optional port, and an ellipsis. Paths and query strings are omitted because they commonly contain the subscription capability.

## Tests

The adapter test suite covers:

- exact and subdomain allowlist matching
- lookalike-host rejection
- scheme, port, credential, fragment, localhost, and private-address rejection
- IPv4, IPv6, mapped-address, CGNAT, benchmarking, documentation, and reserved ranges
- VAPID subject, generated P-256 key, and custom allowlist validation
- VAPID authorization construction
- ECE encryption with generated receiver subscription key material and non-empty ciphertext overhead
- invalid subscription key rejection
- payload mapping and size limits
- deterministic topic generation
- endpoint redaction
- response classification
- rejection before cryptographic or network work

The replacement dependency graph is committed in `Cargo.lock`; CI and container builds use the locked graph rather than resolving dependencies during validation. Advisory and license findings must be remediated or narrowly justified from exact diagnostic evidence; broad security-policy exclusions are not acceptable.

The merge gate requires locked formatting, Clippy with warnings denied, all tests, the Rust 1.88 container build, cargo-deny, RustSec, and full-history Gitleaks to pass on the same reviewed commit.
