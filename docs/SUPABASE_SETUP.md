# Supabase project configuration (manual)

These steps happen in the Supabase **cloud dashboard** for the Sonus Auris
project — they can't be done in app code, and passwordless + MFA + SMS won't work
until they're set. The local `supabase/config.toml` in `sonus-auris-interfaces`
already mirrors most of this for local dev; the cloud project must match.

## 1. Passwordless email (magic link + code)

- **Auth → Providers → Email**: enable Email, keep "Confirm email" as your policy
  allows. Email OTP length **6**, expiry ~1h (matches the clients).
- **Auth → Email Templates → Magic Link**: use a template that includes **both**
  the link and the code, so desktop/console users can type the code. The token
  variable is `{{ .Token }}` — mirror
  `sonus-auris-interfaces/supabase/templates/magic_link.html`. Without
  `{{ .Token }}` in the template, the "enter the 6-digit code" flow has no code to
  enter.
- **Auth → URL Configuration**: add the console + web-server origins to
  **Redirect URLs** (e.g. `https://console.sonusauris.app/*`, the web-server
  origin) so magic-link clicks land back in-app.
- Production SMTP: configure a real SMTP sender (Auth → Email → SMTP) — the shared
  Supabase mailer is rate-limited and not for production volume.

## 2. Multi-factor auth

- **Auth → Multi-Factor**: enable **TOTP** (authenticator app) enroll + verify.
- Enable **Phone** MFA enroll + verify (requires the MFA phone add-on on paid
  plans).
- MFA (beyond the free trial of TOTP) requires the **Supabase Pro** plan — confirm
  the project is on it before relying on enrollment in production.

## 3. SMS provider (for phone OTP / phone MFA)

- **Auth → Providers → Phone**: configure an SMS provider (Twilio is what
  `config.toml` is wired for). Set the Twilio account SID / message service SID in
  the dashboard and the auth token as the project secret
  `SUPABASE_AUTH_SMS_TWILIO_AUTH_TOKEN` (never commit it).
- For local/dev, `config.toml` uses `[auth.sms.test_otp]` so no real SMS is sent.

## 4. Schema & RLS

- Apply `sonus-auris-interfaces/supabase/migrations/20260717180000_devices_entitlements.sql`
  (see FOLLOWUPS 1.3). It creates `devices` (owner-CRUD) and `entitlements`
  (select-only for users).
- Confirm the service role can write `entitlements` (it bypasses RLS) and that
  `authenticated` has only `SELECT` — the reviewed `REVOKE/GRANT` block is in the
  migration.

## 5. Keys for each surface

| Surface | Key | Where |
| --- | --- | --- |
| Mobile app, console, web-server (all clients) | anon / publishable | build config / `--dart-define` (guarded against secret keys at runtime) |
| api-server billing processors only | **service_role** | k8s secret, server-only — writes `entitlements` |

Never place the service-role key in any client build or app repo.
