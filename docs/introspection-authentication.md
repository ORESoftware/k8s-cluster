# Authenticated token introspection

`POST /auth/introspect` returns the complete verified Shared Auth claim set, including stable subject, session, provider provenance, email, identity-system roles, authentication assurance, and the optional authoritative `auth_time`. It is therefore a service-to-service API, not an end-user bearer-token capability.

## Required caller credential

The server exposes introspection only when `AUTH_INTROSPECT_SECRET` contains at least 32 bytes. Callers send that independent secret as the request bearer; the token being inspected remains in the JSON body. It must not be reused as the caller credential.

When `AUTH_INTROSPECT_SECRET` is absent, the endpoint returns `401 Unauthorized`. Missing and incorrect caller credentials return the same response before the supplied token is parsed or verified.

## Freshness and product authorization

For AAL2 tokens, `auth_time` is the verified time of the factor ceremony, not token issue or refresh time. Consumers reject future, stale, or missing values according to their risk policy. AAL1 tokens omit it.

Product tenancy and domain authorization remain the consumer's responsibility. Quaestor Ledger uses `sub` and the live session result from introspection, then resolves tenant membership and financial scopes from Quaestor's own database. Provider metadata and Shared Auth roles do not create a billing grant.

## Rotation and deployment

Treat the credential as narrowly scoped secret material shared only with services that require claims-based authorization. Rotate it through the deployment secret manager. Never place it in browser, mobile, desktop, CLI, analytics, or logging configuration.
