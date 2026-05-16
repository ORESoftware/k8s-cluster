# `remote/auth-server-rs`

Tiny Rust PIN auth service for the EC2 Kubernetes runtime gateway.

The public gateway exposes:

```text
GET  /auth?return=/desired/path
POST /auth
```

Submitting the configured PIN sets:

```text
Set-Cookie: dd_auth=<configured-cookie-value>; Path=/; HttpOnly; SameSite=Lax; Secure
```

The NGINX gateway accepts either the legacy `Auth` request header or this `dd_auth` cookie when the
value matches the configured gateway secret. For protected browser navigations, the gateway
redirects directly to `/auth?return=<original path>`. For non-browser/API callers it keeps the JSON
response:

```json
{ "error": "unauthorized", "errMessage": "missing required dd header" }
```

## Env

| Variable               | Kubernetes source        | Purpose                              |
| ---------------------- | ------------------------ | ------------------------------------ |
| `HOST`                 | Deployment literal       | Bind host.                           |
| `PORT`                 | Deployment literal       | Bind port.                           |
| `DD_AUTH_PIN`          | `dd-remote-auth-secrets` | Operator PIN.                        |
| `DD_AUTH_COOKIE_NAME`  | Deployment literal       | Cookie name trusted by the gateway.  |
| `DD_AUTH_COOKIE_VALUE` | `dd-remote-auth-secrets` | Cookie value trusted by the gateway. |

This is bootstrap auth only. Long-term, replace it with SSO or identity-aware proxy auth and signed
service tokens.
