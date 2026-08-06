# Canonical Plus Shared Auth overlay

This overlay deploys the customer-realm Shared Auth authority used by
`app.canonical.plus`. It is intentionally separate from Fiducia, OreSoftware,
and privileged administrator authorities even though all deployments use the
same container image.

## Browser flow

The public gateway maps `https://app.canonical.plus/shared-auth/*` to this
service and strips the `/shared-auth` prefix before forwarding. The marketing
CTA links to `https://app.canonical.plus/u/quote`; the Cloudflare Worker sends
unauthenticated HTML requests to:

```text
/shared-auth/auth/browser/sign-in?return=/u/quote
```

The server emails a single-use link whose callback is
`/shared-auth/auth/browser/consume`. Successful link or email-OTP verification
sets three host-only cookies:

- `__Host-canonical-customer-auth`: short-lived access JWT;
- `__Host-canonical-customer-auth-refresh`: rotating refresh token;
- `__Host-canonical-customer-auth-emails`: authenticated-encrypted list of at
  most five previously verified email addresses for the sign-in dropdown.

All cookies are `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, and omit
`Domain`. The remembered-email cookie is populated only from the identity
returned by an atomically consumed credential—not from a form field or query
parameter.

## Required secret-store contract

Provision these exact Fiducia/ClusterSecretStore keys before Argo CD sync:

```text
dd/shared-auth/customer/canonical-plus/supabase-projects
dd/shared-auth/customer/canonical-plus/supabase-project-ref
dd/shared-auth/customer/canonical-plus/signing-key-pem
dd/shared-auth/customer/canonical-plus/database-url
dd/shared-auth/customer/canonical-plus/database-endpoint-host
dd/shared-auth/customer/canonical-plus/database-resource-ref
dd/shared-auth/customer/canonical-plus/redis-url
dd/shared-auth/customer/canonical-plus/webhook-secret
dd/shared-auth/customer/canonical-plus/introspect-secret
dd/shared-auth/customer/canonical-plus/browser-seal-secret
dd/shared-auth/customer/canonical-plus/provider-credentials
```

`provider-credentials` may contain the environment variables referenced by the
Supabase metadata plus:

```text
AUTH_SENDGRID_API_KEY
AUTH_OTP_PEPPER
AUTH_EMAIL_FROM
AUTH_TWILIO_ACCOUNT_SID
AUTH_TWILIO_AUTH_TOKEN
AUTH_TWILIO_VERIFY_SERVICE_SID
AUTH_TOTP_ENCRYPTION_KEY
AUTH_WEBAUTHN_RP_ID
AUTH_WEBAUTHN_RP_ORIGIN
AUTH_WEBAUTHN_RP_NAME
```

The browser seal secret and OTP pepper must be unrelated random values of at
least 32 bytes. The database credential must use the non-owner, non-BYPASSRLS
Shared Auth runtime role and a realm-specific database/schema. Never reuse the
Canonical application database owner credential.

## MFA

Magic link/email OTP establishes base assurance. The existing factor API then
supports:

- Twilio Verify SMS OTP after a phone number is verified;
- TOTP enrollment through standard `otpauth://` data, compatible with 3FA and
  ordinary authenticator applications;
- WebAuthn/passkey registration and authentication.

Raw OTPs, TOTP seeds, passkey ceremony state, refresh tokens, phone numbers, and
email addresses must remain absent from logs. Product services authorize using
Shared Auth `acr`, `amr`, `auth_time`, roles, audience, and scope claims; they do
not verify factor secrets themselves.

## Render check

```bash
kubectl kustomize deploy/k8s/overlays/canonical-plus >/tmp/shared-auth-canonical-plus.yaml
kubectl apply --dry-run=server -f /tmp/shared-auth-canonical-plus.yaml
```
