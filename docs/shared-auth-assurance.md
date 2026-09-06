# Shared Auth assurance at the Sonus registration boundary

Sonus Auris accepts Shared Auth only through authenticated server-side introspection. The introspection service credential never enters browser, mobile, or desktop clients.

Numeric `aal = 2` is necessary but not sufficient. Device registration additionally requires:

- `acr = urn:oresoftware:loa:2`;
- a token issued no more than 15 minutes ago, with 60 seconds of future clock skew;
- a passwordless or federated primary method;
- an independent strong second factor: TOTP, SMS OTP, or passkey/WebAuthn;
- no password method anywhere in the authentication chain.

Email OTP cannot satisfy both factors by itself. Missing, duplicated, oversized, malformed, stale, or ambiguous AMR context fails closed with the same MFA-required response and is never logged.

This policy is separate from product authorization. The stable verified Shared Auth `sub` remains the only external identity key used for registration; provider provenance never selects an account or tenant.
