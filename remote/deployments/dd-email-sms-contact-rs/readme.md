# dd-email-sms-contact-rs

Long-lived contact server for the remote runtime: sends **email** (SendGrid; SES-ready) and **SMS**
(Twilio) with per-process rate limiting and a shared-secret auth gate. Axum + `/healthz` + `/readyz`
+ graceful shutdown, matching the other remote Rust servers.

## Interfaces

**HTTP** (port 8120; `/send/*` require header `x-server-auth: $SERVER_AUTH_SECRET` when set):
- `GET  /healthz` — liveness
- `GET  /readyz` — readiness; reports which transports are configured
- `POST /send/email` — `{ "to", "subject", "html", "text"?, "from"? }`
- `POST /send/sms` — `{ "to", "body" }`

**NATS** (when `NATS_URL` is set) — subjects from `remote/libs/nats/subject-defs` (crate
`dd-nats-subject-defs`), consumed via queue group `dd-email-sms-contact` (each request handled once
across replicas):
- `dd.remote.contact.email.send` (subscribe) — `{ to, subject, html, [text], [from] }`
- `dd.remote.contact.sms.send` (subscribe) — `{ to, body }`
- `dd.remote.contact.results` (publish) — per-send result summary `{ ok, channel, to, transport, upstreamStatus, error, rateLimited }`

## Env

| Var | Purpose |
|---|---|
| `NATS_URL` | enables the NATS consumer (e.g. `nats://dd-nats.messaging.svc.cluster.local:4222`) |
| `SENDGRID_API_KEY` | **must have the `mail.send` scope** (an admin/management key returns 401 "not authorized to send mail") |
| `EMAIL_FROM` | verified SendGrid sender, e.g. `outreach@dancingdragons.cc` |
| `TWILIO_ACCOUNT_SID` / `TWILIO_AUTH_TOKEN` / `TWILIO_FROM_NUMBER` | SMS via Twilio |
| `SERVER_AUTH_SECRET` | shared secret for the HTTP `/send/*` gate |
| `EMAIL_RATE_PER_MIN` / `SMS_RATE_PER_MIN` | token-bucket caps (default 60 / 30) |
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
