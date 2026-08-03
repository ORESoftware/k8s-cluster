# Redirect OAuth production gate

Stripe, PayPal, and Braintree redirect callbacks arrive from the provider without the initiating user's Shared Auth bearer. The current callback stores a subject and ceremony timestamp in one-time state and rechecks Quaestor membership, but it cannot prove that the initiating Shared Auth session is still active when credentials are persisted. A session revoked after flow initiation can therefore remain usable until the state or step-up window expires.

Production keeps `BILLING_REDIRECT_OAUTH_ENABLED=false`. The server refuses to boot with this flag enabled unless `BILLING_ALLOW_INSECURE_DEV=1`, and both redirect start and callback handlers fail closed while disabled. Plaid's authenticated frontend exchange is not affected.

The production replacement is a two-phase flow:

1. The provider callback consumes state, exchanges the code, seals the credential into a short-lived pending record, and returns no active connection.
2. The authenticated frontend calls a one-time finalization endpoint with its current Shared Auth bearer.
3. Quaestor introspects that bearer, requires the same `sub`, a live `sid`, fresh AAL2, exact tenant membership, and `billing:write`, then atomically activates the pending credential and records an audit event.
4. Pending records expire quickly, are single-use, never expose provider tokens in URLs, and are deleted after success or expiry.

Do not enable redirect OAuth in production until that finalization path, expiry cleanup, replay tests, revocation tests, and provider-code error handling are merged.
