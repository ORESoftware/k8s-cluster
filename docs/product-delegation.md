# Product token delegation

`POST /auth/delegate` is the only supported way for one OreSoftware product to
obtain a user-bound token for another product. It is not an unrestricted token
minting API.

## MemeBank to ClipTown policy

MemeBank authenticates users through shared-auth. It does not call the 3FA
backend, import a 3FA server SDK, validate an external 3FA proof, check whether a
3FA mobile app is installed, or deep-link into 3FA as part of authorization.
TOTP, passkey, email OTP, SMS OTP, and compatible 3FA-imported factors are
verified by shared-auth and appear only as bounded `aal`, `acr`, `amr`, and
`auth_time` claims.

ClipTown API calls use a delegated token with:

- `aud = cliptown-api`
- `azp = memebank-api`
- a new `jti`
- `parent_jti` identifying the base-token lineage without embedding the bearer
- the same stable `sub` and revocation-aware `sid`
- the same provider provenance, roles, `aal`, `acr`, `amr`, and `auth_time`
- only the configured `cliptown:memebank:*` scopes
- an expiry no later than either the policy TTL or the parent-token expiry

A delegated token cannot be delegated again.

## Configuration

`AUTH_DELEGATION_POLICIES` is a JSON array. An empty or absent array makes the
endpoint deny every exchange.

```json
[
  {
    "client_id": "memebank-api",
    "audience": "cliptown-api",
    "allowed_scopes": [
      "cliptown:memebank:read",
      "cliptown:memebank:write",
      "cliptown:memebank:delete"
    ],
    "require_loa2_scopes": [
      "cliptown:memebank:write",
      "cliptown:memebank:delete"
    ],
    "required_roles": ["user"],
    "ttl_secs": 300,
    "max_auth_age_secs": 600
  }
]
```

The parser rejects unknown fields, duplicate client/audience tuples, duplicate or
malformed scopes, LOA2 scopes that are not in the allowed set, excessive TTLs,
and unbounded policy arrays. Changes should be deployed through the normal
secret/configuration controller rather than a command-line flag.

## Request

```http
POST /auth/delegate
Authorization: Bearer <base shared-auth token>
Content-Type: application/json

{
  "client_id": "memebank-api",
  "audience": "cliptown-api",
  "scopes": ["cliptown:memebank:read"]
}
```

The `client_id` identifies a public product client and is not treated as a
secret. Security comes from the authenticated user token, exact allow-list,
short lifetime, narrow scope, downstream resource ownership checks, and session
revocation.

For sensitive scopes, shared-auth requires LOA2 and a recent `auth_time`. It does
not require or accept a product-specific factor-app header. This lets users
satisfy policy with any configured method while preventing MemeBank from
coupling itself to 3FA installation or availability.

## Downstream verification

ClipTown must verify the ES256 signature through shared-auth JWKS and pin all of:

- issuer
- `aud = cliptown-api`
- `azp = memebank-api`
- expiry and not-before
- the delegated token's own `jti` and its distinct `parent_jti`
- active `sid`
- required `cliptown:memebank:*` scope
- resource ownership for `sub`
- LOA/freshness for sensitive routes

Protected `/auth/introspect` accepts an optional exact `audience` field for
services that choose revocation-aware remote introspection. An active response
includes both `jti` and `parent_jti`, allowing a resource server to distinguish
the current delegated grant from its parent without receiving either bearer.
The service credential belongs only on introspection and must never be forwarded
to MemeBank, ClipTown clients, factor endpoints, or delegated API calls.
