# Follow-ups — what still needs finishing

Status as of 2026-07-18. Each item lists where the foundation already is, what's
missing, and concrete acceptance criteria. Priorities:

- **P0** — a shipped feature is incomplete or a claim isn't actually enforced.
- **P1** — needed before the paid tier / MFA can be relied on end to end.
- **P2** — polish, coverage, and consistency.

Paths are relative to each app's repo (see the [apps table](../README.md)).

---

## P0 — the paid tier is not enforced anywhere server-side

Entitlements and the 2-device limit are modelled and gated **client-side only**.
Nothing on a server rejects an over-limit device or an unpaid upgrade, so today
the gate is advisory.

### 0.1 Billing verification bodies (api-server)
`sonus-auris-api-server.rs/src/main.rs` + `src/billing.rs` register five routes,
all returning `501` with the intended contract documented in each handler:

| Route | Must do |
| --- | --- |
| `POST /api/v1/billing/stripe/webhook` | Verify the `Stripe-Signature` HMAC with `STRIPE_WEBHOOK_SECRET`; on `checkout.session.completed` / `customer.subscription.*`, read `client_reference_id` (the Supabase user id the console passes) and upsert `public.entitlements` (`plan=plus`, `device_limit>=3`, `source=stripe`, `external_ref=<subscription id>`, `current_period_end`). |
| `POST /api/v1/billing/app-store/notifications` | Verify the App Store Server Notifications **v2** JWS (Apple root certs); map via the app account token; upsert `entitlements` (`source=app_store`). |
| `POST /api/v1/billing/play/rtdn` | Verify the Play RTDN Pub/Sub message; map via the purchase token; upsert `entitlements` (`source=play_store`). |
| `POST /api/v1/billing/app-store-receipt` | Client-submitted receipt (mobile) → verify with Apple → upsert. |
| `POST /api/v1/billing/play-purchase` | Client-submitted purchase token (mobile) → verify with Google → upsert. |

**Writes use the Supabase service-role key** (bypasses RLS; `entitlements` is
select-only for users). Required env: `STRIPE_WEBHOOK_SECRET`, `SUPABASE_URL`,
`SUPABASE_SERVICE_ROLE_KEY` (see [HARDENING.md](./HARDENING.md) for secret
handling). Make every processor **idempotent** (dedupe on event id / original
transaction id) — webhooks retry.

**Acceptance:** a completed Stripe Checkout writes a `plus` row the console reads
back on its next poll; replaying the same event is a no-op.

### 0.2 The mobile IAP endpoints don't exist yet
`sonus-auris-ui.dart/lib/src/services/billing_service.dart` POSTs store proof to
`POST /api/mobile/v1/billing/app-store-receipt` and `/play-purchase`. Those
routes are **not in `sonus-auris-backend.rs`** — the client treats the `404` as
"pending server support" and logs it. Either add them to the backend or route
mobile there via the api-server (0.1). Until then, a mobile purchase completes in
the store but grants nothing.

### 0.3 Server-side device-limit enforcement
The limit (`entitlements.device_limit`, 2 on free) is enforced only as a client
soft-gate:
- mobile: `device_registry.dart::selectDeviceIdsOverLimit` → an "upgrade" banner,
  recording still runs.
- console: `device_service.dart::overLimitDeviceIds` → the stalest recorders
  render locked.

A user can bypass both by editing the client. Add server-side enforcement in the
backend upload path: reject presign/upload for a device that is over the account
limit (recompute the same ordering the clients use), and reject registration of
the (N+1)th active recorder. The DB check constraint only pins `free ⇒ ≤ 2`; it
does **not** count devices.

**Acceptance:** a 3rd recorder on a free account cannot obtain upload URLs.

### 0.4 Revoked-device enforcement
Mobile checks its own `devices.revoked_at` and halts cloud sync
(`app_controller.dart`), but the backend still accepts uploads bearing that
device's token. The backend should reject upload sessions from a device whose
`revoked_at` is set.

---

## P1 — auth is not end-to-end until these land

### 1.1 Mobile passwordless UI swap
The controller + client are done and tested (`requestSupabaseEmailOtp`,
`confirmSupabaseEmailOtp`, MFA passthroughs in `app_controller.dart`), but the
**UI still collects a password**: `lib/src/widgets/supabase_auth_form.dart`,
`supabase_auth_panel.dart`, and the `_AccountSection` in `lib/main.dart` still use
`obscureText` fields and call `signInWithPassword` / `signUpWithSupabase`.

Swap them to the email-code flow the console already implements
(`sonus-auris-web-desktop.dart/lib/src/ui/sign_in_screen.dart` is the reference):
email → 6-digit code → optional MFA challenge. Keep the client's password methods
for now but remove them from the UI. Update `supabase_auth_form_test.dart` /
`supabase_auth_panel_test.dart` to the new flow.

**Acceptance:** no `obscureText`/password field remains under `lib/`; onboarding
completes with only an emailed code.

### 1.2 Web-server second-factor challenge screen
`sonus-auris-web-server.rs`: the GoTrue MFA client is implemented and unit-tested
(`src/auth.rs::mfa_challenge` / `mfa_verify` / `access_token_aal`,
`MfaFactor`) but **gated with `#[allow(dead_code)]`**, and `otp_verify` only
*logs* the aal1-with-verified-factors case (`TODO(web-mfa)` in `src/main.rs`),
then creates the session anyway.

To finish: after `verify_email_otp`, if `access_token_aal == "aal1"` and the user
has a verified factor, **do not** create the browser session yet — stash the aal1
tokens in a short-lived encrypted cookie (reuse `src/crypto.rs`), render a
challenge page (Maud), add `POST /auth/mfa/challenge` + `POST /auth/mfa/verify`,
and only `sessions.create` once `mfa_verify` returns an aal2 session. Remove the
`#[allow(dead_code)]` markers as each item gets wired.

**Acceptance:** a web sign-in for an MFA-enrolled account cannot reach
`/dashboard` without passing a factor.

### 1.3 Apply the devices/entitlements migration
`sonus-auris-interfaces/supabase/migrations/20260717180000_devices_entitlements.sql`
is committed but **not applied** (the interfaces repo applies migrations as a
manual, reviewed step — see its `AGENTS.md`). Apply it to the Sonus Auris
Supabase project, then confirm RLS (`devices` owner-CRUD, `entitlements`
select-only) with the live cross-user test the mobile repo already has.

### 1.4 Supabase project configuration (dashboard, not code)
See [SUPABASE_SETUP.md](./SUPABASE_SETUP.md). Passwordless + MFA + SMS won't work
until TOTP/phone MFA are enabled, an SMS provider is configured, and the
magic-link email template carries `{{ .Token }}`.

---

## P2 — coverage, consistency, ops

- **Push the new repos.** `sonus-auris-web-desktop.dart` and
  `sonus-auris-api-server.rs` have `origin` remotes configured but the GitHub
  repos may not exist / aren't pushed; the monorepo pins reference local commits.
  Create the repos, push, then re-verify the monorepo submodule pins resolve.
- **CI secrets.** proxy `deploy.yml` needs `CLOUDFLARE_API_TOKEN` (Workers
  Scripts: Edit); monorepo `integration` CI needs `SUBMODULE_SSH_KEY` for the
  private submodules; api-server/console CI are self-contained.
- **WebSocket integration test.** `sonus-auris-web-server.rs` unit-tests the
  change-gate hash but not the `/ws` stream loop (auth-before-upgrade, OOB push,
  close on revocation). Add an axum `WebSocket` integration test.
- **Console device-mutation widget tests.** rename/revoke/delete are
  controller-tested (`console_controller_test.dart`) but not driven through the
  `devices_screen.dart` dialogs.
- **E2E against a deployed console.** the Puppeteer/Playwright smokes honor
  `CONSOLE_BASE_URL` and there's a `k8s-job.yaml` for the cluster's browser
  runners, but no console URL is deployed yet. Deploy the web build and point the
  cluster job at it (or wire it into `~/codes/ores/k8s-cluster/remote/tests`).
- **Dedicated Postgres namespace.** Tables currently live in the Supabase
  `public` schema of the sonus-auris project. If a dedicated schema/namespace is
  wanted beyond project isolation, move them and update the generator's
  `schema:` field + RLS/grants accordingly.
