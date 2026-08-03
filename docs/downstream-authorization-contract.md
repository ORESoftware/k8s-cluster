# Downstream authentication and authorization contract

Shared Auth establishes identity and authentication assurance. Product services remain responsible for authorizing access to their own tenants and resources.

## Stable token fields

Downstream services may rely on the following signed claims after verifying the Shared Auth issuer, audience, signature, expiry, and not-before constraints:

- `sub`: stable Shared Auth user identifier.
- `sid`: revocable Shared Auth session identifier. Production services that require immediate revocation should use the authenticated introspection path in addition to local JWT verification.
- `roles`: server-controlled grants loaded from Shared Auth Postgres.
- `aal` and `acr`: authentication assurance. Unknown or missing values are base assurance, never AAL2.
- `amr`: normalized authentication methods.
- `auth_time`: Unix seconds when the current AAL2 assurance was established. It is omitted from AAL1 tokens.

A product that requires recent step-up authentication must require all of the following:

1. `aal == 2` or `acr == "urn:oresoftware:loa:2"`;
2. a present `auth_time` that is not in the future beyond normal clock skew;
3. an application-defined maximum age for `now - auth_time`;
4. the product-specific role or scope needed for the operation.

A refreshed session returns to AAL1. Refresh never extends a previous AAL2 ceremony indefinitely.

## Provider metadata is not product tenancy

`provider`, `provider_tenant`, and `provider_subject` describe the external identity that authenticated the user. In particular, `provider_tenant` may be a Supabase project or another identity-provider namespace. Product services must not interpret it as a customer, organization, account, or billing tenant.

## Product-local authorization

Shared Auth roles describe identity-system grants only. Customer tenancy, billing roles, financial scopes, invoice authority, and reconciliation authority belong to each product's own database and audit log. They must not be copied into user-writable identity metadata or inferred from provider provenance.

Quaestor Ledger therefore resolves `sub` from Shared Auth, then loads the exact tenant membership and billing scopes from Quaestor's database. Shared Auth does not mint `quaestor:*` tenant grants. A product-local role or scope is an authorization input, never authentication assurance; it cannot imply AAL2 or a recent step-up.

## Verification posture

Services should verify Shared Auth JWTs locally against the pinned issuer, audience, allowed algorithm, and JWKS endpoint. This keeps the hot path available during a transient Shared Auth network outage. Services that require immediate logout or account-disable propagation should additionally call authenticated introspection, fail closed when the revocation authority is unavailable, and bound any positive cache by the token expiry and the product's risk tolerance.

Static process-wide bearer tokens are migration credentials only. They carry no user, tenant, assurance, or role and must not authorize tenant-scoped financial mutations.
