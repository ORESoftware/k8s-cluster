# `dd-sound-recorder-rs`

Rust backend for a mobile sound-recorder "dashcam" product. It serves the public product pages,
device registration, rolling audio segment upload sessions, S3 presigned upload URLs, and
short-lived evidence export download links. Users can also link user-owned cloud storage
destinations so completed segments are mirrored out of the centralized S3 bucket.

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
- Registration records platform, install id, consent version, consent timestamp, and acknowledgement
  that the client exposes an active recording indicator.
- The rolling retention cap defaults to 500 hours and is enforced in API queries. S3 lifecycle rules
  should also expire `sound-recorder/segments/*` objects at the bucket layer.

## Routes

- `GET /` — public product page.
- `GET /privacy` — privacy posture page.
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
| `SOUND_RECORDER_S3_BUCKET` / `S3_BUCKET` | unset | Required for presigned upload/download URLs. |
| `SOUND_RECORDER_S3_KEY_PREFIX` | `sound-recorder/segments` | Object key prefix. |
| `SOUND_RECORDER_CDN_BASE_URL` | unset | Optional CloudFront/base URL returned as `cdnUrl`. |
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
| `SOUND_RECORDER_GOOGLE_CLIENT_ID` / `SOUND_RECORDER_GOOGLE_CLIENT_SECRET` | unset | OAuth client for Google Drive `drive.file` links. |
| `SOUND_RECORDER_MICROSOFT_CLIENT_ID` / `SOUND_RECORDER_MICROSOFT_CLIENT_SECRET` | unset | OAuth client for Microsoft OneDrive AppFolder links. |
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

## Local Smoke

```bash
cd remote/deployments/dd-sound-recorder-rs
SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION=true \
SOUND_RECORDER_DEVICE_TOKEN_PEPPER=local-dev-pepper \
SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= \
SOUND_RECORDER_SERVER_AUTH_SECRET=local-dev-secret \
cargo run
```

The page, health, metrics, and generated docs render without cloud credentials. Mobile write paths
need Postgres tables from `remote/libs/pg-defs/schema/schema.sql` plus S3 credentials.
