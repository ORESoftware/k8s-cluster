<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/dd-sound-recorder-rs` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/sonus-auris/sonus-auris-backend.rs` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/dd-sound-recorder-rs`.
<!-- END k8s-cluster-submodule-notice --># sonus-auris-backend.rs (`dd-sound-recorder-rs`)

Backend for **Sonus Auris** — the audio-dashcam product. The Cargo crate / binary is named
`dd-sound-recorder-rs` (kept stable so the cluster image, service, and deployment manifests do not
churn). This is a self-contained crate: build it from this repo root with `cargo build --release`.

It is consumed by the `ores/k8s-cluster` repo as a git submodule mounted at
`remote/deployments/dd-sound-recorder-rs`; the runtime clones the superproject with
`--recurse-submodules` and `cargo run`s from that path. See that repo's
`remote/argocd/dd-next-runtime/dd-sound-recorder-rs.deployment.yaml`.

Rust backend for a mobile sound-recorder "dashcam" product. It serves the public product pages,
device registration, rolling audio segment upload sessions, S3 presigned upload URLs, and
short-lived evidence export download links. Users can also link user-owned cloud storage
destinations so completed segments are mirrored out of the centralized S3 bucket.
It also accepts mobile alert events and can email a timestamped listening link that starts before
the event.

## Shape

- Mobile clients record short audio segments locally and request a new presigned S3 `PUT` URL for
  each segment.
- The service stores metadata in Postgres and stores audio bytes in S3. It does not proxy audio
  through the Rust process.
- Google Drive and Microsoft OneDrive links use server-side OAuth tokens sealed with AES-256-GCM.
  The server stores only sealed token envelopes in Postgres and refreshes access tokens inside the
  internal copy drain.
- Apple iCloud is client-managed because Apple does not expose a general server-side iCloud Drive
  OAuth/write API. The backend tracks the linked iCloud destination and exposes copy jobs with
  short-lived S3 download URLs for the iOS client to copy into its iCloud/CloudKit container.
- CloudFront belongs on the playback/download side. Uploads are presigned S3 `PUT`s; evidence
  exports use short-lived S3 `GET` URLs until a CloudFront-signing layer is added.
- Device auth uses opaque bearer tokens. Tokens are returned only on registration and stored as
  SHA-256 hashes with a server-side pepper.
- Account identity is rooted in Supabase. When `register` is called with a verified Supabase access
  token in the `x-supabase-auth` header, the account is keyed to the token's `sub`
  (`external_subject = supabase:<sub>`), which is the only way to attach a device to an existing
  account. A shared `SOUND_RECORDER_REGISTRATION_BEARER` still works for trusted server-to-server
  callers and may assert an arbitrary `externalSubject`; public registration keys the account to the
  install id and ignores any client-supplied `externalSubject`, so an anonymous caller can never
  claim another user's account.
- Google Drive / OneDrive links support a hybrid OAuth flow: the client may pass Supabase-brokered
  `providerAccessToken`/`providerRefreshToken` to `cloud-connections/oauth/complete` to be sealed
  directly, or omit them to fall back to the server-side authorization-code exchange.
- Upload sessions carry a `useCase` (`security` default, `music`, `meeting`, `voice_note`,
  `ambient`) and an optional `audioProfile` (sensitivity, treble/mid/bass gain, channel layout)
  stored with session metadata, so the same backend serves dashcam and musician capture.
- Registration records platform, install id, consent version, consent timestamp, and acknowledgement
  that the client exposes an active recording indicator.
- The rolling retention cap defaults to 500 hours and is enforced in API queries. S3 lifecycle rules
  should also expire `sound-recorder/segments/*` objects at the bucket layer.

## Routes

- `GET /` — public product page.
- `GET /privacy` — privacy posture page.
- `GET /listen/:alert_id` — short-lived audio alert listening page.
- `GET /download/ios` — redirects to `SOUND_RECORDER_IOS_APP_STORE_URL`.
- `GET /download/android` — redirects to `SOUND_RECORDER_ANDROID_PLAY_STORE_URL`.
- `POST /api/mobile/v1/devices/register` — creates or rotates a device token.
- `POST /api/mobile/v1/upload-sessions` — starts a device upload session.
- `POST /api/mobile/v1/upload-sessions/:session_id/segments/presign` — creates/refreshes one
  segment row and returns a presigned S3 `PUT`.
- `POST /api/mobile/v1/upload-sessions/:session_id/segments/:segment_id/complete` — marks a
  segment uploaded after the mobile client receives success from S3.
- `POST /api/mobile/v1/upload-sessions/:session_id/heartbeat` — refreshes session liveness and
  returns the next expected sequence number.
- `POST /api/mobile/v1/upload-sessions/:session_id/close` — closes an upload session.
- `GET /api/mobile/v1/timeline` — lists uploaded segment metadata inside the rolling retention
  window.
- `POST /api/mobile/v1/evidence-exports` — returns short-lived download links for an account/time
  range and writes an audit row.
- `POST /api/mobile/v1/permanent-saves` — pins uploaded segments (by `segments[].storageKey` or by
  `rangeStartedAt`/`rangeEndedAt`) so the retention sweep never expires them. Returns the pinned
  storage keys.
- `POST /api/mobile/v1/alerts` — creates a short evidence export around a mobile trigger and
  optionally posts an email payload to `SOUND_RECORDER_ALERT_EMAIL_WEBHOOK_URL`. Alerts are
  accepted only when uploaded retained segments overlap the requested listening window.
- `GET /api/mobile/v1/cloud-connections` — lists linked user cloud destinations.
- `POST /api/mobile/v1/cloud-connections/oauth/start` — starts a Google Drive, OneDrive, or
  client-managed iCloud link flow.
- `POST /api/mobile/v1/cloud-connections/oauth/complete` — completes a link, seals OAuth tokens
  for server-managed providers, and backfills recent uploaded segments into copy jobs.
- `POST /api/mobile/v1/cloud-connections/:connection_id/revoke` — revokes a linked destination,
  clears sealed credentials, and skips pending copy jobs.
- `GET /api/mobile/v1/cloud-copy-jobs` — lists iCloud client-managed copy jobs with short-lived
  S3 download links.
- `POST /api/mobile/v1/cloud-copy-jobs/:job_id/complete` — marks a client-managed cloud copy
  complete.
- `POST /internal/retention/sweep` — server-authenticated marker sweep for expired segment rows.
- `POST /internal/cloud-copy/drain` — server-authenticated worker drain for pending Google Drive
  and OneDrive copy jobs.
- `GET /healthz`, `GET /readyz`, `GET /metrics`.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`.

## Environment

| Var | Default | Notes |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind host. |
| `PORT` | `8126` | Bind port. |
| `SOUND_RECORDER_RDS_DATABASE_URL` | falls back to shared RDS env vars | Postgres URL. |
| `SOUND_RECORDER_PG_POOL_MAX_SIZE` | `16` | Max pooled Postgres connections (clamped to `1..100`). Connections are pooled and reused, not opened per request. |
| `SOUND_RECORDER_S3_BUCKET` / `S3_BUCKET` | unset | Required for presigned upload/download URLs. |
| `SOUND_RECORDER_S3_KEY_PREFIX` | `sound-recorder/segments` | Object key prefix. |
| `SOUND_RECORDER_CDN_BASE_URL` | unset | Optional CloudFront/base URL returned as `cdnUrl`. |
| `SOUND_RECORDER_PUBLIC_BASE_URL` | unset | HTTPS base URL used to build `/listen/:alert_id` links in alert emails. HTTP is allowed only for localhost development. |
| `SOUND_RECORDER_ALERT_EMAIL_TO` | `alexander.d.mills@gmail.com` | Server-controlled alert recipient. Client-supplied recipients are ignored. |
| `SOUND_RECORDER_ALERT_EMAIL_WEBHOOK_URL` | unset | Optional webhook that receives `{ to, subject, text, html }` for alert emails. |
| `SOUND_RECORDER_DEVICE_TOKEN_PEPPER` | local random fallback | Required for durable device-token verification. |
| `SOUND_RECORDER_REGISTRATION_BEARER` | unset | Optional bearer required by device registration. |
| `SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION` | `false` | Explicitly opens registration when no bearer is configured. |
| `SOUND_RECORDER_SERVER_AUTH_SECRET` / `SERVER_AUTH_SECRET` | unset | Required for `/internal/retention/sweep`. |
| `SOUND_RECORDER_DEFAULT_RETENTION_HOURS` | `500` | Clamped to `1..500`. |
| `SOUND_RECORDER_DEFAULT_SEGMENT_SECONDS` | `60` | Suggested mobile segment length. |
| `SOUND_RECORDER_MAX_SEGMENT_SECONDS` | `120` | Upper bound accepted by the API. |
| `SOUND_RECORDER_MAX_SEGMENT_BYTES` | `10485760` | Upper bound accepted by the API. |
| `SOUND_RECORDER_UPLOAD_URL_TTL_SECONDS` | `300` | Short-lived S3 PUT URL TTL. |
| `SOUND_RECORDER_DOWNLOAD_URL_TTL_SECONDS` | `900` | Short-lived evidence GET URL TTL. |
| `SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY` | unset | Base64-encoded 32-byte AES-GCM key required for server-managed Google Drive and OneDrive links. |
| `SOUND_RECORDER_SUPABASE_URL` / `SUPABASE_URL` | unset | Supabase project URL. Used to derive the JWKS URL and expected issuer. |
| `SOUND_RECORDER_SUPABASE_JWT_SECRET` / `SUPABASE_JWT_SECRET` | unset | Legacy HS256 JWT secret. Enables verifying HS256 Supabase tokens. |
| `SOUND_RECORDER_SUPABASE_JWKS_URL` | `${SUPABASE_URL}/auth/v1/.well-known/jwks.json` | JWKS endpoint for asymmetric (RS256/ES256) Supabase signing keys. Cached for one hour. |
| `SOUND_RECORDER_SUPABASE_ISSUER` | `${SUPABASE_URL}/auth/v1` | Expected `iss` claim. |
| `SOUND_RECORDER_SUPABASE_AUDIENCE` | `authenticated` | Expected `aud` claim. |
| `SOUND_RECORDER_GOOGLE_CLIENT_ID` / `SOUND_RECORDER_GOOGLE_CLIENT_SECRET` | unset | OAuth client for Google Drive `drive.file` links. |
| `SOUND_RECORDER_MICROSOFT_CLIENT_ID` / `SOUND_RECORDER_MICROSOFT_CLIENT_SECRET` | unset | OAuth client for Microsoft OneDrive AppFolder links. |
| `SOUND_RECORDER_GOOGLE_AUTHORIZATION_URL` / `SOUND_RECORDER_GOOGLE_TOKEN_URL` | Google OAuth endpoints | Optional provider endpoint overrides for local integration tests. |
| `SOUND_RECORDER_GOOGLE_DRIVE_UPLOAD_URL` | Google Drive upload endpoint | Optional upload endpoint override for local integration tests. |
| `SOUND_RECORDER_MICROSOFT_AUTHORIZATION_URL` / `SOUND_RECORDER_MICROSOFT_TOKEN_URL` | Microsoft consumer OAuth endpoints | Optional provider endpoint overrides for local integration tests. |
| `SOUND_RECORDER_MICROSOFT_GRAPH_BASE_URL` | Microsoft Graph v1.0 endpoint | Optional Graph endpoint override for local integration tests. |
| `SOUND_RECORDER_OAUTH_STATE_TTL_SECONDS` | `600` | OAuth link state TTL, clamped to `60..3600`. |
| `SOUND_RECORDER_CLOUD_COPY_BATCH_SIZE` | `25` | Internal copy drain batch size, clamped to `1..100`. |
| `SOUND_RECORDER_CLOUD_COPY_MAX_ATTEMPTS` | `3` | Retry attempts before a server-managed copy job is marked failed. |
| `SOUND_RECORDER_CLOUD_COPY_MAX_BYTES` | `26214400` | Per-segment server-managed copy byte limit, clamped to `1..209715200`. |
| `SOUND_RECORDER_CLOUD_BACKFILL_SEGMENTS` | `240` | Uploaded retained segments to enqueue when a cloud destination is linked. |
| `SOUND_RECORDER_IOS_APP_STORE_URL` | unset | `/download/ios` target. |
| `SOUND_RECORDER_ANDROID_PLAY_STORE_URL` | unset | `/download/android` target. |

`/readyz` requires Postgres, S3, durable token pepper, registration posture, and internal auth to be
configured. `/healthz` always reports process health and configuration booleans.

## Mobile Notes

The app stores should be treated as part of the product contract, not a deploy afterthought. Mobile
clients need a visible active-recording state, clear onboarding consent, user controls to stop
recording and export/delete data, and jurisdiction-aware guidance because recording consent laws vary.
On Android, the recorder will likely need a microphone foreground service. On iOS, background audio
capture must fit Apple's background-audio rules and review expectations. For iCloud mirroring, the
iOS client must use Apple-approved iCloud/CloudKit APIs and report copy completion back to the
backend because the server cannot directly write to a user's arbitrary iCloud Drive account.

Alert listen pages include all matching segment download URLs and advance through them in order.
The mobile app should still be treated as the primary smooth playback surface because it has the
sample-counted segment timeline and can trim intentional overlap with a gapless playlist.

## Local Smoke

```bash
SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION=true \
SOUND_RECORDER_DEVICE_TOKEN_PEPPER=local-dev-pepper \
SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= \
SOUND_RECORDER_SERVER_AUTH_SECRET=local-dev-secret \
cargo run
```

The page, health, metrics, and generated docs render without cloud credentials. Mobile write paths
need the Postgres tables (schema lives in the `ores/k8s-cluster` monorepo under
`remote/libs/pg-defs/schema/schema.sql`) plus S3 credentials. The `migrations/` directory here is
applied out-of-band — see `migrations/RUNBOOK.md`.
