# `remote/auth-server-rs`

Tiny Rust PIN auth service for the EC2 Kubernetes runtime gateway.

The public gateway exposes:

```text
GET  /auth?return=/desired/path
POST /auth
```

Submitting the configured operator passphrase sets:

```text
Set-Cookie: dd_auth=<configured-cookie-value>; Path=/; Max-Age=3600; HttpOnly; SameSite=Lax; Secure
```

The NGINX gateway accepts either the legacy `Auth` request header or this `dd_auth` cookie when the
value matches the configured gateway secret. For protected browser navigations, the gateway
redirects directly to `/auth?return=<original path>`. For non-browser/API callers it keeps the JSON
response:

```json
{ "error": "unauthorized", "errMessage": "missing required dd header" }
```

## Env

| Variable                         | Kubernetes source        | Purpose                              |
| -------------------------------- | ------------------------ | ------------------------------------ |
| `HOST`                           | Deployment literal       | Bind host.                           |
| `PORT`                           | Deployment literal       | Bind port.                           |
| `DD_AUTH_PIN`                    | `dd-remote-auth-secrets` | Operator passphrase.                 |
| `DD_AUTH_COOKIE_NAME`            | Deployment literal       | Cookie name trusted by the gateway.  |
| `DD_AUTH_COOKIE_VALUE`           | `dd-remote-auth-secrets` | Cookie value trusted by the gateway. |
| `DD_AUTH_COOKIE_MAX_AGE_SECONDS` | Deployment literal       | Browser auth session TTL. Defaults to `3600`; capped at one day. |
| `DD_AUTH_TOTP_SECRET_BASE32`     | `dd-remote-auth-secrets` | Optional base32 TOTP seed. When set, login requires passphrase plus a current six-digit code. |
| `DD_AUTH_TOTP_WINDOW_STEPS`      | Deployment literal       | Optional TOTP clock-skew window. Defaults to `1`; capped at `2`. |

This is still bootstrap auth, but the optional TOTP seed makes the "first human inquiry" unlock a
real two-factor browser session instead of a static cookie. Long-term, replace it with SSO or
identity-aware proxy auth and signed service tokens.
