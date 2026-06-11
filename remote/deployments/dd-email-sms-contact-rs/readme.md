# dd-email-sms-contact-rs

Long-lived contact server for the remote runtime: sends **email** (SendGrid; SES-ready), **SMS**
(Twilio), and **push notifications** (Web Push/VAPID, Firebase Cloud Messaging, Expo, Apple APNs)
with per-process rate limiting and a shared-secret auth gate. Axum + `/healthz` + `/readyz` +
graceful shutdown, matching the other remote Rust servers.

## Interfaces

**HTTP** (port 8120; `/send/*` require header `x-server-auth: $SERVER_AUTH_SECRET` when set):
- `GET  /healthz` — liveness
- `GET  /readyz` — readiness; reports which transports are configured
- `POST /send/email` — `{ "to", "subject", "html", "text"?, "from"? }`
- `POST /send/sms` — `{ "to", "body" }`
- `POST /send/push` — `{ "transport", "title"?, "body"?, "data"?, "token"?, "subscription"? }`

**NATS** (when `NATS_URL` is set) — subjects from `remote/libs/nats/subject-defs` (crate
`dd-nats-subject-defs`), consumed via queue group `dd-email-sms-contact` (each request handled once
across replicas):
- `dd.remote.contact.email.send` (subscribe) — `{ to, subject, html, [text], [from] }`
- `dd.remote.contact.sms.send` (subscribe) — `{ to, body }`
- `dd.remote.contact.push.send` (subscribe) — `{ transport, [title], [body], [data], [token], [subscription] }`
- `dd.remote.contact.results` (publish) — per-send result summary `{ ok, channel, to, transport, upstreamStatus, error, rateLimited }`

### Push payload

`transport` selects the backend; the target shape depends on it. At least one of `title`/`body` is
required; `data` is an optional object (FCM coerces its values to strings; APNs places its keys
alongside `aps`; Web Push/Expo pass it through).

- `webpush` — needs `subscription: { endpoint, keys: { p256dh, auth } }` (a browser `PushSubscription`).
- `fcm` / `expo` / `apns` — need a device `token` (FCM registration token, `ExponentPushToken[…]`, or
  APNs device token respectively).

A transport responds `503 … not configured` until its credentials are present (Expo needs none, so it
is always live). `/readyz` reports per-transport readiness under `push`.

### Hardening

The `webpush` `endpoint` is caller-supplied and the NATS lane has no auth gate, so an unrestricted
client would be an **SSRF** primitive into the cluster. The endpoint is validated before any network
call or rate-limit token is spent: it must be `https` on port 443, must carry no embedded credentials,
must not be an IP literal in a private/loopback/link-local/CGNAT range (blocks `169.254.169.254` and
friends even when the allowlist is open), and its host must match `WEBPUSH_ALLOWED_HOSTS` (default: the
known browser push services — FCM, Mozilla autopush, WNS, Apple). In the opt-in open mode
(`WEBPUSH_ALLOWED_HOSTS=*`) the host is additionally **resolved** and rejected if any answer is an
internal address (this catches names like `metadata.google.internal` or a domain pointed at
`127.0.0.1`); note that pure DNS rebinding is not fully closed in open mode, so the default host
allowlist remains the real boundary.

Other controls:
- Device tokens (FCM/Expo/APNs) are validated — bounded length and no URL-significant or control
  characters — so a token can't break out of the APNs request path (`/3/device/{token}`).
- The `/send/push` route caps the request body at 64 KiB; payloads are capped at 8 KiB.
- Upstream error text is truncated to 1 KiB before it is returned or published onto the results bus.
- Result summaries carry only a redacted target (token prefix or `scheme://host/…`) — never the full
  device token or per-subscription endpoint path.

## Env

| Var | Purpose |
|---|---|
| `NATS_URL` | enables the NATS consumer (e.g. `nats://dd-nats.messaging.svc.cluster.local:4222`) |
| `SENDGRID_API_KEY` | **must have the `mail.send` scope** (an admin/management key returns 401 "not authorized to send mail") |
| `EMAIL_FROM` | verified SendGrid sender, e.g. `outreach@dancingdragons.cc` |
| `TWILIO_ACCOUNT_SID` / `TWILIO_AUTH_TOKEN` / `TWILIO_FROM_NUMBER` | SMS via Twilio |
| `VAPID_PRIVATE_KEY` / `VAPID_SUBJECT` | Web Push: EC P-256 private key PEM + contact subject (default `mailto:outreach@dancingdragons.cc`) |
| `WEBPUSH_ALLOWED_HOSTS` | comma-separated host suffixes the webpush endpoint may target; `*` opens it to any public host (private/loopback IPs still blocked). Default: known push services |
| `WEBPUSH_TTL_SECONDS` | how long a push service holds an undelivered webpush message (default 43200 = 12h) |
| `FCM_SERVICE_ACCOUNT_JSON` / `FCM_PROJECT_ID` | FCM HTTP v1: full service-account JSON; project id falls back to the JSON's `project_id` |
| `APNS_KEY_P8` / `APNS_KEY_ID` / `APNS_TEAM_ID` / `APNS_TOPIC` / `APNS_USE_SANDBOX` | Apple APNs token auth (.p8 PEM, key id, team id, app bundle id; `APNS_USE_SANDBOX=1` for the sandbox host) |
| `EXPO_ACCESS_TOKEN` | Expo: optional, only if push security is enabled on the project |
| `SERVER_AUTH_SECRET` | shared secret for the HTTP `/send/*` gate |
| `EMAIL_RATE_PER_MIN` / `SMS_RATE_PER_MIN` / `PUSH_RATE_PER_MIN` | token-bucket caps (default 60 / 30 / 60) |
| `HOST` / `PORT` | bind (default `0.0.0.0:8120`) |

## Secrets

Send credentials come from the `dd-email-sms-contact-secrets` k8s Secret (External Secrets →
AWS Secrets Manager `dd/remote-dev/email-sms-contact-secrets`). The deployment marks every key
`optional: true`, so the pod boots and reports readiness before the bundle is populated; `/readyz`
shows which transports are live. **Populate that AWS secret with a SendGrid key that has the
`mail.send` scope** (the dd-next-1 `.env.local` key is admin-only and cannot send).

## Build / deploy

Built in-cluster like the other Rust servers: `rust:1.91.1-bookworm` clones the `k8s-cluster` repo
at pod start and runs `cargo run --release --locked` from this directory. Registered in
`remote/argocd/dd-next-runtime/kustomization.yaml`; ArgoCD syncs on push to `dev`.

The Web Push transport pulls `web-push` → `ece`, which links `openssl-sys` against the system libssl.
That is already present in `rust:*-bookworm` (it is built on `buildpack-deps:bookworm`, which ships
`libssl-dev` + `pkg-config`), so no image change is needed — but a switch to a `-slim`/Alpine base
would require adding those packages. Everything else stays pure-Rust/rustls.
