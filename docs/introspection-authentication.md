# Authenticated token introspection

`POST /auth/introspect` returns the complete verified Shared Auth claim set, including stable subject, session, provider provenance, email, roles, and authentication assurance. It is therefore a service-to-service API, not an end-user bearer-token capability.

## Required caller credential

The server exposes introspection only when `AUTH_INTROSPECT_SECRET` contains at least 32 bytes. Callers must send that independent secret in the request authorization header:

```http
Authorization: Bearer <AUTH_INTROSPECT_SECRET>
Content-Type: application/json

{"token":"<shared-auth access token>"}
```

The token being inspected remains in the JSON body. It must not be reused as the caller credential.

When `AUTH_INTROSPECT_SECRET` is absent, the endpoint returns `401 Unauthorized`. Missing and incorrect caller credentials also return the same response, before the supplied token is parsed or verified.

## Rotation and deployment

Treat the credential as a narrowly scoped secret shared only by services that require claims-based authorization. Rotate it through the deployment secret manager and update callers before removing the previous value from a coordinated rollout. Never place it in browser, mobile, desktop, CLI, analytics, or logging configuration.

Services that only need a bearer validity decision should use `GET /auth/verify` or local JWKS verification instead of introspection. Product tenancy and domain authorization remain the consumer's responsibility and must not be inferred from provider provenance such as `provider_tenant`.
