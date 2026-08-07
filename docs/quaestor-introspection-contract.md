# Quaestor Ledger introspection contract

Quaestor Ledger uses `POST /auth/introspect` as the revocation-aware authentication boundary for tenant-scoped financial operations. The caller authenticates with an independent service credential; the end-user Shared Auth token remains in the JSON body.

For an active token, Shared Auth returns the stable `sub`, `sid`, issuer, audience, expiry, AAL, ACR, normalized AMR methods, and optional `auth_time`. `auth_time` is present only when AAL2 was established from a server-owned ceremony or a verified upstream factor timestamp. It is not token issue time.

Quaestor must fail closed when introspection is unavailable, inactive, malformed, from the wrong issuer or audience, missing a required session identifier, or claiming AAL2 without a usable `auth_time`. Quaestor then resolves tenant membership and financial scopes from its own database; Shared Auth provider metadata and roles never establish a billing tenant grant.

Roll out the provider change before enabling `BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS`. During rollout, probe an actual AAL2 token and require `active=true`, `aal=2`, the LOA2 ACR, an accepted AMR method, and a bounded `auth_time` before routing financial mutations to Quaestor.

## Required release evidence

Record one successful AAL1 exchange, one successful AAL2 exchange, and negative cases for absent/wrong introspection credentials, inactive tokens, revoked sessions, missing session identifiers, stale/future `auth_time`, and authority unavailability. Evidence must contain status codes, bounded timestamps, and correlation identifiers, but never access tokens, service credentials, factor secrets, or provider credentials.
