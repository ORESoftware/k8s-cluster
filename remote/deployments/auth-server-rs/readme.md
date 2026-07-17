# `remote/deployments/auth-server-rs`

Tiny Rust PIN auth service for the EC2 Kubernetes runtime gateway.

The public gateway exposes:

```text
GET  /auth?return=/desired/path     -> login form (also shows current cookie state)
POST /auth                          -> validate passphrase + optional TOTP, set cookie
GET  /auth/status                   -> JSON { authenticated, totpRequired, cookieName }
```

Submitting the configured operator passphrase sets:

```text
Set-Cookie: dd_auth=<configured-cookie-value>; Path=/; Max-Age=259200; HttpOnly; SameSite=Lax; Secure
```

The NGINX gateway accepts either the legacy `Auth` request header or this `dd_auth` cookie when the
value matches the configured gateway secret. For protected browser navigations, the gateway
redirects directly to `/auth?return=<original path>`. For non-browser/API callers it keeps the JSON
response:

```json
{ "error": "unauthorized", "errMessage": "missing required dd header" }
```

## UX feedback

The login form gives the operator an explicit signal in three places, so a submission is never
ambiguous:

- The `GET /auth?return=...` form renders a "✓ You are currently signed in" or "You are not
  currently signed in" banner based on the request's `dd_auth` cookie.
- A successful `POST /auth` renders a "✓ Logged in successfully. Browser cookie was set." page
  with a 2-second meta-refresh to `return`, plus a manual "Continue now" button. The `Set-Cookie`
  header is attached to that response body so the cookie is established before the redirect.
- A failed `POST /auth` re-renders the form with a red error banner and a 401 status. When
  `DD_AUTH_TOTP_SECRET_BASE32` is set, the form also marks the "One-time code" field as
  `(required — 6-digit TOTP)`; otherwise it is marked `(not required — leave blank)` so operators
  don't waste time hunting for a TOTP they never enrolled.

Scripts and curl callers that prefer the original immediate redirect can post `immediate=1`:

```bash
curl -sS -i -X POST https://<gateway>/auth \
  --data-urlencode "pin=$DD_AUTH_PIN" \
  --data-urlencode "totp=$(oathtool --totp -b "$DD_AUTH_TOTP_SECRET_BASE32")" \
  --data-urlencode "return_to=/home" \
  --data "immediate=1"
```

This responds with `303 See Other`, the same `Location: /home` header, and the `Set-Cookie` header.

## Probing auth state

`GET /auth/status` is unauthenticated metadata about the current request and the deployment:

```json
{ "authenticated": true, "totpRequired": true, "cookieName": "dd_auth" }
```

Use it from the browser to confirm a cookie is live (`fetch('/auth/status').then(r => r.json())`),
or from scripts to verify that a generated session cookie is valid before sending downstream
requests.

## Env

| Variable                         | Kubernetes source        | Purpose                              |
| -------------------------------- | ------------------------ | ------------------------------------ |
| `HOST`                           | Deployment literal       | Bind host.                           |
| `PORT`                           | Deployment literal       | Bind port.                           |
| `DD_AUTH_PIN`                    | `dd-remote-auth-secrets` | Operator passphrase.                 |
| `DD_AUTH_COOKIE_NAME`            | Deployment literal       | Cookie name trusted by the gateway.  |
| `DD_AUTH_COOKIE_VALUE`           | `dd-remote-auth-secrets` | Cookie value trusted by the gateway. |
| `DD_AUTH_COOKIE_MAX_AGE_SECONDS` | Deployment literal       | Browser auth session TTL in seconds. Defaults to `259200` (3 days); capped at 3 days. |
| `DD_AUTH_TOTP_SECRET_BASE32`     | `dd-remote-auth-secrets` | Optional base32 TOTP seed. When set, login requires passphrase plus a current six-digit code. |
| `DD_AUTH_TOTP_WINDOW_STEPS`      | Deployment literal       | Optional TOTP clock-skew window. Defaults to `1`; capped at `2`. |

This is still bootstrap auth, but the optional TOTP seed makes the "first human inquiry" unlock a
real two-factor browser session instead of a static cookie. Long-term, replace it with SSO or
identity-aware proxy auth and signed service tokens.
