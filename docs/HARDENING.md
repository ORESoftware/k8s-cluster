# Hardening — how to shore things up

Robustness, security, and supply-chain notes across the stack. These are not
bugs; they're the "make it production-solid" list. Companion to
[FOLLOWUPS.md](./FOLLOWUPS.md) (functional gaps).

---

## Secrets & keys

- **Never ship a privileged Supabase key to a client.** Clients enforce this at
  request time (`supabase_key_policy.dart` in both Flutter apps,
  `key_policy.dart` in the console) — they reject `sb_secret_*` and
  `service_role` JWTs. Keep it that way; only the anon/publishable key belongs in
  `--dart-define` / build config.
- **Service-role key is server-only.** The billing processors (api-server) are
  the only things that write `entitlements`, using
  `SUPABASE_SERVICE_ROLE_KEY`. Store it (and `STRIPE_WEBHOOK_SECRET`, Twilio
  creds) as k8s secrets pulled at deploy from the `~/codes/ores/k8s-cluster`
  gitops repo — never in an app repo, `.cli-flags.toml`, or an image layer.
- **Rotate the web-server session key.** `sonus-auris-web-server.rs` encrypts
  Supabase tokens at rest (`crypto.rs`, AES-256-GCM) under
  `session_encryption_key`. Confirm it's a per-environment secret, not the test
  value baked in the config default.

## Payments compliance (keep the store rails separate)

- **Mobile = store IAP only.** `billing_service.dart` uses `in_app_purchase` and
  posts store proof to the backend; it has **no** external payment path and never
  self-grants (`onEntitlementsShouldRefresh` re-reads after the server writes).
  Do not add a Stripe/web-checkout link to the mobile app — Apple 3.1.1 / Google
  Play Payments forbid it for digital goods.
- **Console Stripe = web/desktop only.** `billing_screen.dart` hides the Stripe
  path on native store binaries (`platform_info.dart::isNativeMobileStoreBinary`)
  and shows "manage in the mobile app". Keep that guard; if the console is ever
  packaged for an app store, the guard must fire.
- **Entitlements are server truth.** No client path may set `plan`/`device_limit`
  — RLS makes `entitlements` select-only. Verify this holds after applying the
  migration (1.3 in FOLLOWUPS).

## Web / browser surface

- **Proxy is the header enforcement point.** `sonusauris-app-proxy` re-serves the
  security headers GitHub Pages drops, canonicalizes hosts, and allowlists
  request/response headers. Keep `dist/_headers` in the site repo and the
  proxy's `SECURITY_HEADERS` **in sync** — they will drift silently otherwise
  (the proxy README calls this out).
- **Consider self-hosting htmx + the ws extension.** The web-server loads htmx
  and `htmx-ext-ws` from jsdelivr, SRI-pinned and CSP-allowlisted. SRI protects
  integrity, but self-hosting the two files under a `/static` route removes the
  third-party runtime dependency and lets you tighten `script-src` to `'self'`.
- **Console web token storage.** On web the console keeps the GoTrue session in
  `shared_preferences` (browser storage), not a secret vault — acceptable
  because it's a short-lived token the server revalidates every call, but
  document it and prefer an httpOnly-cookie-backed session if the web build ever
  holds anything longer-lived. Native desktop already uses the OS keychain.
- **WebSocket auth.** `/ws` authenticates via the session cookie **before**
  upgrading and closes on server-side revocation. Keep auth-before-upgrade; never
  accept an unauthenticated socket and gate later.

## Data & privacy

- **Zero-knowledge audio stays zero-knowledge.** Audio is sealed on-device before
  upload; the multi-device master-viewer path wraps each segment key to the
  account X25519 key (`crypto/account_recipient.dart`). If/when the console gains
  audio playback, it must decrypt on-device behind the PIN — never send keys or
  plaintext server-side.
- **RLS everywhere.** Every `public` table has owner-scoped RLS; the interfaces CI
  asserts it. Any new table must ship with RLS + a reviewed `REVOKE/GRANT` block
  (the generator emits the policy; grants are manual — see the interfaces
  migrations for the pattern).
- **Telemetry/consent are append-only.** `client_telemetry` and `user_consents`
  are insert+select only by RLS so history can't be rewritten. Preserve that when
  editing `schema/tables.json`.

## Reliability / ops

- **Idempotent billing.** All webhook/notification processors must dedupe on the
  provider event id (webhooks retry; a double-apply must not double-grant).
- **Backend upload retries already survive crashes** (segments persisted as
  `uploading` are retried) — keep that invariant when touching the upload drain.
- **Proxy timeouts.** Upstream fetches abort at 15s → 504; keep a bound so a slow
  origin can't pin Worker time.
- **Submodule pins are the deploy record.** Bump the monorepo pin (and the
  k8s-cluster pin of the monorepo) only after each app repo is pushed, so a
  cluster SHA always resolves to real, pushed commits.

## Test / CI gaps to close

- Web-server `/ws` stream loop has no integration test (only the hash gate).
- Console device mutations aren't widget-tested end to end.
- E2E smokes run against the local `build/web`; add a scheduled run against a
  deployed console (`CONSOLE_BASE_URL`) on the cluster's Playwright runners.
- The monorepo `integration` CI checks the vendored-interfaces copies are the
  same contract (whitespace-normalized) — keep vendoring in lockstep when
  `schema/tables.json` changes: regenerate, re-copy `generated/dart/` into both
  Flutter apps, bump pins.
