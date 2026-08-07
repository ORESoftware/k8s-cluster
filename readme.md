> [!IMPORTANT]
> **Deprecated.** This backend has been consolidated into
> [sonus-auris-api-server.rs](https://github.com/sonus-auris/sonus-auris-api-server.rs),
> including this repository's Git history. The consolidation is tracked in
> [API PR #7](https://github.com/sonus-auris/sonus-auris-api-server.rs/pull/7).
> Do not start new work here. Cut the existing deployment and submodule over to
> the canonical API repository before archiving this repository.

<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Legacy deployment source.** Until the deployment cutover is complete, this
> repository remains vendored as a git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/dd-sound-recorder-rs`. Do not make new feature changes in
> either checkout; migrate the deployment to the canonical API repository.
>
> On disk: source clone `~/codes/sonus-auris/sonus-auris-backend.rs` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/dd-sound-recorder-rs`.
<!-- END k8s-cluster-submodule-notice -->

# sonus-auris-backend.rs (`dd-sound-recorder-rs`)

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

## Repository layout

This crate is deliberately small on disk. Each meaningful folder has its own `README.md`.

- [`src/`](./src/README.md) — a thin binary plus library modules for SeaORM
  persistence, OpenTelemetry/Loki telemetry, and the Axum domain service.
- [`migrations/`](./migrations/README.md) — reviewed, copy-pasteable reference SQL and the
  [`RUNBOOK.md`](./migrations/RUNBOOK.md). Schema is declared authoritatively in the
  `ores/k8s-cluster` monorepo (`remote/libs/pg-defs/schema/schema.sql`) and applied out-of-band,
  not by this process.
- [`generated/`](./generated/README.md) — generated, checked-in API documentation
  (`api-docs.json` / `api-docs.html`) served at `/api/docs`. Do not hand-edit.

The crate is also packaged with Nix (`flake.nix`, `.nix/`) and a `Dockerfile` for the cluster image.

## Shape

- Mobile clients record short audio segments locally and request a new presigned S3 `PUT` URL for
  each segment.
- The service stores metadata in Postgres and audio bytes in the configured AWS S3 / Cloudflare R2
  primary backend. Normal mobile transfer is direct via presigned URLs; the Rust process reads
  bytes only for explicit server-managed Google Drive / OneDrive copy jobs.
- Google Drive, Microsoft OneDrive, and Dropbox links use authorization-code
  OAuth with S256 PKCE, offline refresh access, and server-side tokens sealed
  with AES-256-GCM. OneDrive uses the Microsoft `common` tenant by default so
  both personal and work/school accounts are supported when enabled on the app
  registration.
  The server stores only sealed token envelopes in Postgres and refreshes access tokens inside the
  internal copy drain.
- Apple iCloud is client-managed because Apple does not expose a general server-side iCloud Drive
  OAuth/write API. The backend tracks the linked iCloud destination and exposes copy jobs with
  short-lived S3 download URLs for the iOS client to copy into its iCloud/CloudKit container.
- User-owned Amazon S3 and Cloudflare R2 destinations are also client-managed.
  Their credentials remain in the device secure store; the backend records only
  safe connection status, display name, and folder path.
- Postgres is authoritative for connection lifecycle. A durable outbox projects
  safe owner-scoped status into Supabase `public.cloud_connections`; OAuth
  tokens, storage credentials, provider subject IDs, and arbitrary metadata are
  never included in that projection.
- CloudFront belongs on the playback/download side. Uploads are presigned S3 `PUT`s; evidence
  exports use short-lived S3 `GET` URLs until a CloudFront-signing layer is added.
- Device auth uses opaque bearer tokens. Tokens are returned only on registration and stored as
  SHA-256 hashes with a server-side pepper.
- Account identity is rooted in the RDS-backed shared-auth authority. When
  `register` receives a shared-auth access token in `x-shared-auth`, the backend
  introspects it and keys the account to the verified UUID
  (`external_subject = shared-auth:<sub>`). AAL2 is required by default, so an
  email magic link/OTP must be followed by verified SMS MFA before enrolling a
  device.
- Supabase remains a secondary identity path. A verified token in
  `x-supabase-auth` uses `external_subject = supabase:<sub>`, but empty
  Supabase configuration and a short Supabase outage do not affect existing
  device sessions or backend readiness. A shared
  `SOUND_RECORDER_REGISTRATION_BEARER` still works for trusted
  server-to-server callers and may assert an arbitrary `externalSubject`;
  public registration keys the account to the install id and ignores any
  client-supplied `externalSubject`, so an anonymous caller can never claim
  another user's account.
- `DELETE /api/mobile/v1/account` remains the Supabase-specific deletion path:
  it verifies the signed-in user's JWT, deletes Sonus Auris backend metadata,
  revokes device/cloud tokens, and deletes the Supabase Auth user with the
  server-only service-role key.
- Browser account data stays a JSON concern here: the typed `/api/v1/data/*` routes verify the
  caller's Supabase JWT and forward that JWT plus the publishable key to the Supabase Data API.
  `sonus-auris-interfaces` deserializes the response and Supabase RLS remains the row-authorization
  boundary; these routes never use the service-role key.
- Google Drive / OneDrive / Dropbox links support a hybrid OAuth flow: the client may pass Supabase-brokered
  `providerAccessToken`/`providerRefreshToken` to `cloud-connections/oauth/complete` to be sealed
  directly, or omit them to fall back to the server-side authorization-code exchange.
- Cloud-copy delivery uses Google Drive resumable uploads in aligned 8 MiB
  chunks, OneDrive's single-call AppFolder upload within the service's 200 MiB
  ceiling, and Dropbox upload sessions above its 150 MiB single-call limit.
  Provider requests have a separate bounded upload timeout and never forward a
  Google bearer token to the provider-issued resumable-session URL.
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
- `GET /api/v1/data/acoustic-events?limit=50` — returns up to 200 owner-scoped `AcousticEvent`
  rows using the shared interface crate and the caller's Supabase access token.
- `GET /api/v1/data/user-consents?limit=50` — returns up to 200 owner-scoped `UserConsent` rows
  using the same JWT/RLS path. Both list routes return `{ "count": N, "data": [...] }`.
- `GET /api/v1/data/user-settings` — returns the typed owner settings row, or mobile-compatible
  defaults before the first save.
- `PUT /api/v1/data/user-settings` — validates and upserts the portable settings subset. The owner
  id and update time are server-controlled; credentials and device-only controls are not accepted.
  Browser data routes carry the Supabase JWT in `X-Supabase-Auth: Bearer ...`, intentionally
  separate from the device token accepted in `Authorization` on mobile routes.
- `POST /api/mobile/v1/devices/register` — creates or rotates a device token.
- `POST /api/mobile/v1/devices/heartbeat` — refreshes server-owned `last_seen_at`
  with the opaque device token. Apps call it every ten minutes as the durable
  fallback to Supabase Realtime Presence.
- `GET /api/mobile/v1/devices/presence` — upgrades to the independent Rust
  presence WebSocket. The device token is sent in the first bounded frame,
  never in a URL; open sockets recheck revocation every 30 seconds.
- `POST /api/mobile/v1/devices/:install_id/revoke` — verifies that the install
  is visible through the caller's Supabase device/group RLS view, then
  invalidates every matching Rust device token.
- `GET /api/v1/data/devices?limit=200` — lists devices visible through the
  caller's Supabase account-group RLS view for browser/desktop device screens.
- `DELETE /api/mobile/v1/account` — deletes private storage objects, revokes Sonus Auris metadata
  and credentials, then deletes the signed-in Supabase Auth user. Storage deletion is batched,
  checked, and retry-safe.
- `POST /api/mobile/v1/upload-sessions` — starts a device upload session.
- `POST /api/mobile/v1/upload-sessions/:session_id/segments/presign` — creates/refreshes one
  segment row and returns a presigned S3 `PUT`.
- `POST /api/mobile/v1/upload-sessions/:session_id/segments/:segment_id/complete` — verifies the
  object with authenticated `HeadObject` (existence, size, content type, ETag) before marking it
  uploaded; the mobile client's success report is never trusted by itself.
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
- `POST /api/mobile/v1/cloud-connections/oauth/start` — starts a Google Drive,
  OneDrive, Dropbox, or client-managed iCloud/S3/R2 link flow.
- `GET /oauth/callback` — hosted provider callback that safely forwards the
  one-time OAuth code and state into the waiting Sonus Auris app session.
- `GET /oauth/manual-callback` — hosted Windows/Linux fallback that displays
  the escaped one-time provider code for an explicit paste back into the
  desktop Connections window.
- `POST /api/mobile/v1/cloud-connections/oauth/complete` — completes a link, seals OAuth tokens
  for server-managed providers, and backfills recent uploaded segments into copy jobs.
- `POST /api/mobile/v1/cloud-connections/:connection_id/revoke` — revokes a linked destination,
  makes a five-second best-effort upstream authorization revocation for Google
  Drive/Dropbox, clears sealed credentials regardless of provider availability,
  and skips pending copy jobs. OneDrive credentials are destroyed locally;
  Microsoft requires the user/admin consent portal for grant-wide revocation.
- `GET /api/mobile/v1/cloud-copy-jobs` — lists iCloud client-managed copy jobs with short-lived
  S3 download links.
- `POST /api/mobile/v1/cloud-copy-jobs/:job_id/complete` — marks a client-managed cloud copy
  complete.
- `POST /internal/retention/sweep` — server-authenticated marker sweep for expired segment rows.
- `POST /internal/cloud-copy/drain` — server-authenticated worker drain for pending Google Drive,
  OneDrive, and Dropbox copy jobs.
- `POST /internal/cloud-connection-projections/drain` — server-authenticated
  outbox drain that upserts safe connection status into Supabase.
- `GET /healthz`, `GET /readyz`, `GET /metrics`.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`.

## Environment

### Auth invariants — do not weaken

- **Shared-auth is primary and server-introspected.** Mobile registration sends
  its access token only in `x-shared-auth`; the backend authenticates to
  `/auth/introspect` with a separate service secret and uses only the returned
  active UUID. Unverified email is discarded, and AAL2 is required by default.
- **Supabase is optional and secondary.** Its absence or outage does not gate
  readiness unless `SOUND_RECORDER_REQUIRE_SUPABASE=true` is explicitly set.

- **A pinned issuer is mandatory.** `SOUND_RECORDER_SUPABASE_ISSUER` (derived
  from `SOUND_RECORDER_SUPABASE_URL` when unset) must resolve, or Supabase auth
  stays off. `aud` is `authenticated` on *every* Supabase project, so `iss` is
  the only claim that binds a token to this one. See `SupabaseConfig::is_enabled`.
- **An unconfirmed email claim is discarded.** The verifier only keeps `email`
  when the token asserts `email_verified` (top-level or under `user_metadata`).
  Without that, an email-keyed decision is only as strong as the project's
  "Confirm email" setting — with it off, anyone can sign up claiming someone
  else's address. The token is still valid; `sub` remains the account identity.

| Var | Default | Notes |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind host. |
| `PORT` | `8126` | Bind port. |
| `SOUND_RECORDER_RDS_DATABASE_URL` | falls back to shared RDS env vars | Postgres URL used by SeaORM. |
| `SOUND_RECORDER_PG_POOL_MAX_SIZE` | `16` | SeaORM Postgres pool size (clamped to `1..100`). |
| `SOUND_RECORDER_S3_BUCKET` / `S3_BUCKET` | unset | Primary AWS S3 bucket. `SOUND_RECORDER_R2_BUCKET` / `R2_BUCKET` are equivalent R2 aliases. |
| `SOUND_RECORDER_S3_KEY_PREFIX` | `sound-recorder/segments` | Object key prefix. |
| `SOUND_RECORDER_S3_REGION` / `R2_REGION` | `us-east-1` (`auto` for R2) | SigV4 region. R2 endpoints are always signed with Cloudflare's required `auto` region. |
| `SOUND_RECORDER_S3_ENDPOINT` / `SOUND_RECORDER_R2_ENDPOINT` / `R2_ENDPOINT` / `S3_ENDPOINT` | unset | HTTPS S3-compatible endpoint. `SOUND_RECORDER_R2_ACCOUNT_ID` can derive Cloudflare's endpoint automatically. |
| `SOUND_RECORDER_S3_ACCESS_KEY_ID` / `SOUND_RECORDER_S3_SECRET_ACCESS_KEY` | SDK credential chain | Optional service-scoped credential pair. R2-specific aliases are also supported; native AWS IAM roles and standard `AWS_*` credentials continue to work. |
| `SOUND_RECORDER_S3_FORCE_PATH_STYLE` | `false` (`true` for generic custom endpoints) | Select path-style addressing when required by MinIO/another compatible store. R2 defaults to its documented virtual-host style. |
| `SOUND_RECORDER_S3_SERVER_SIDE_ENCRYPTION` | `auto` | `auto`, `aes256`, or `none`. Auto signs explicit AES256 only for native AWS S3; it omits the unsupported header for R2, which already encrypts objects at rest. |
| `SOUND_RECORDER_S3_MAX_ATTEMPTS` | `3` | AWS SDK standard retry attempts, clamped to `1..10`, with exponential backoff and jitter. |
| `SOUND_RECORDER_S3_VERSIONING_MODE` | `unversioned` for R2; required for AWS/custom S3 | Must be explicitly `unversioned` (or `disabled`) for AWS/custom S3. Versioned and versioning-suspended buckets are rejected because key-only deletion does not physically erase prior versions. R2 has no versioning. |
| `SOUND_RECORDER_S3_READINESS_OBJECT_KEY` | unset | Existing sentinel inside the configured prefix. Strict readiness performs a bounded remote `HeadObject`; no `HeadBucket`/`ListBucket` permission is needed. Production must set this. |
| `SOUND_RECORDER_ALLOW_SIGNING_ONLY_STORAGE_READINESS` | `false` | Development-only opt-out permitting a local SigV4 signing check when no sentinel is configured. It does not prove remote availability or authorization. |
| `SOUND_RECORDER_ALLOW_UNMARKED_STORAGE_HISTORY` | `false` | Temporary legacy-rollout acknowledgment for rows created before storage fingerprints. Requires the companion fingerprint below. Mismatched marked rows always fail readiness. Backfill verified rows, then return this to `false` before any backend cutover. |
| `SOUND_RECORDER_UNMARKED_STORAGE_HISTORY_FINGERPRINT` | unset | When the legacy acknowledgment is true, this must exactly match `storageBackendFingerprint` from `/healthz`. Changing endpoint/region/bucket invalidates the acknowledgment automatically. |
| `SOUND_RECORDER_CDN_BASE_URL` | unset | Optional CloudFront/base URL returned as `cdnUrl`. |
| `SOUND_RECORDER_PUBLIC_BASE_URL` | unset | HTTPS base URL used to build `/listen/:alert_id` links in alert emails. HTTP is allowed only for localhost development. |
| `SOUND_RECORDER_ALERT_EMAIL_TO` | unset | Server-controlled alert recipient. Alerts fail closed until configured; client-supplied recipients are ignored. |
| `SOUND_RECORDER_ALERT_EMAIL_WEBHOOK_URL` | unset | Optional webhook that receives `{ to, subject, text, html }` for alert emails. |
| `SOUND_RECORDER_DEVICE_TOKEN_PEPPER` | local random fallback | Required for durable device-token verification. |
| `SOUND_RECORDER_REGISTRATION_BEARER` | unset | Optional bearer required by device registration. |
| `SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION` | `false` | Explicitly opens registration when no bearer is configured. |
| `SOUND_RECORDER_SERVER_AUTH_SECRET` / `SERVER_AUTH_SECRET` | unset | Required for retention, cloud-copy, storage-mirror, and cloud-connection projection internal drains. |
| `SOUND_RECORDER_DEFAULT_RETENTION_HOURS` | `500` | Clamped to `1..500`. |
| `SOUND_RECORDER_DEFAULT_SEGMENT_SECONDS` | `60` | Suggested mobile segment length. |
| `SOUND_RECORDER_MAX_SEGMENT_SECONDS` | `120` | Upper bound accepted by the API. |
| `SOUND_RECORDER_MAX_SEGMENT_BYTES` | `10485760` | Upper bound accepted by the API. |
| `SOUND_RECORDER_UPLOAD_URL_TTL_SECONDS` | `300` | Short-lived S3 PUT URL TTL. A segment too near its retention cutoff to leave this TTL plus a ten-minute upload-settle window is rejected rather than risking a post-delete write. |
| `SOUND_RECORDER_DOWNLOAD_URL_TTL_SECONDS` | `900` | Short-lived evidence GET URL TTL. |
| `SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY` | unset | Base64-encoded 32-byte AES-GCM key required for server-managed Google Drive, OneDrive, and Dropbox links. |
| `SOUND_RECORDER_SHARED_AUTH_BASE_URL` | unset | Root URL for the RDS-backed shared-auth authority. HTTPS is required except for loopback or in-cluster HTTP. Empty disables this registration path. |
| `SOUND_RECORDER_SHARED_AUTH_INTROSPECT_SECRET` | unset | 32+ byte service credential for shared-auth introspection. Must be configured with the base URL and never shipped in an app. |
| `SOUND_RECORDER_SHARED_AUTH_REQUIRED_AAL` | `2` | Minimum assurance level for shared-auth device registration. `2` requires verified second-factor SMS/TOTP-equivalent assurance. |
| `SOUND_RECORDER_SUPABASE_URL` / `SUPABASE_URL` | unset | Supabase project URL. Used to derive the JWKS URL and expected issuer. |
| `SOUND_RECORDER_SUPABASE_PUBLISHABLE_KEY` / `SUPABASE_PUBLISHABLE_KEY` | unset | Publishable (or legacy anon) key used with the caller's JWT for typed `/api/v1/data/*` reads. It is not a service-role key. |
| `SOUND_RECORDER_SUPABASE_JWT_SECRET` / `SUPABASE_JWT_SECRET` | unset | Legacy HS256 JWT secret. Enables verifying HS256 Supabase tokens. |
| `SOUND_RECORDER_SUPABASE_JWKS_URL` | `${SUPABASE_URL}/auth/v1/.well-known/jwks.json` | JWKS endpoint for asymmetric (RS256/ES256) Supabase signing keys. It must share the project URL's origin and is cached for at most ten minutes. |
| `SOUND_RECORDER_SUPABASE_ISSUER` | `${SUPABASE_URL}/auth/v1` | Expected `iss` claim. **Required for Supabase auth to be enabled at all.** `aud` is the literal `authenticated` on every Supabase project, so `iss` is the only claim binding a token to *this* project; without it the verifier is not built and token-authenticated routes report unavailable rather than accepting tokens from any project. Setting `SOUND_RECORDER_SUPABASE_URL` derives it automatically — a setup that configures only a raw `SOUND_RECORDER_SUPABASE_JWT_SECRET` (local/dev) must set this variable explicitly. |
| `SOUND_RECORDER_SUPABASE_AUDIENCE` | `authenticated` | Expected `aud` claim. |
| `SOUND_RECORDER_SUPABASE_SERVICE_ROLE_KEY` / `SUPABASE_SERVICE_ROLE_KEY` | unset | Server-only Supabase service-role key. Required for account deletion and the safe `cloud_connections` projection; never expose it to an app. |
| `SOUND_RECORDER_REQUIRE_SUPABASE` | `false` | Opt-in readiness gate for complete Supabase account support. Keep false when Supabase is a secondary auth/projection service. |
| `SOUND_RECORDER_GOOGLE_CLIENT_ID` / `SOUND_RECORDER_GOOGLE_CLIENT_SECRET` | unset | OAuth client for Google Drive `drive.file` links. |
| `SOUND_RECORDER_MICROSOFT_CLIENT_ID` / `SOUND_RECORDER_MICROSOFT_CLIENT_SECRET` | unset | OAuth client for Microsoft OneDrive AppFolder links. |
| `SOUND_RECORDER_DROPBOX_CLIENT_ID` / `SOUND_RECORDER_DROPBOX_CLIENT_SECRET` | unset | OAuth client for Dropbox app-folder links using `files.content.write`. |
| `SOUND_RECORDER_GOOGLE_AUTHORIZATION_URL` / `SOUND_RECORDER_GOOGLE_TOKEN_URL` | Google OAuth endpoints | Optional provider endpoint overrides for local integration tests. |
| `SOUND_RECORDER_GOOGLE_DRIVE_UPLOAD_URL` | Google Drive upload endpoint | Optional upload endpoint override for local integration tests. |
| `SOUND_RECORDER_MICROSOFT_AUTHORIZATION_URL` / `SOUND_RECORDER_MICROSOFT_TOKEN_URL` | Microsoft `common` OAuth endpoints | Optional provider endpoint overrides for local integration tests. |
| `SOUND_RECORDER_MICROSOFT_GRAPH_BASE_URL` | Microsoft Graph v1.0 endpoint | Optional Graph endpoint override for local integration tests. |
| `SOUND_RECORDER_DROPBOX_AUTHORIZATION_URL` / `SOUND_RECORDER_DROPBOX_TOKEN_URL` | Dropbox OAuth endpoints | Optional provider endpoint overrides for local integration tests. |
| `SOUND_RECORDER_DROPBOX_UPLOAD_URL` | Dropbox content upload endpoint | Optional upload endpoint override for local integration tests. |
| `SOUND_RECORDER_OAUTH_STATE_TTL_SECONDS` | `600` | OAuth link state TTL, clamped to `60..3600`. |
| `SOUND_RECORDER_CLOUD_COPY_BATCH_SIZE` | `25` | Internal copy drain batch size, clamped to `1..100`. |
| `SOUND_RECORDER_CLOUD_COPY_MAX_ATTEMPTS` | `3` | Retry attempts before a server-managed copy job is marked failed. |
| `SOUND_RECORDER_CLOUD_COPY_MAX_BYTES` | `26214400` | Per-segment server-managed copy byte limit, clamped to `1..209715200`. |
| `SOUND_RECORDER_CLOUD_BACKFILL_SEGMENTS` | `240` | Uploaded retained segments to enqueue when a cloud destination is linked. |
| `SOUND_RECORDER_IOS_APP_STORE_URL` | unset | `/download/ios` target. |
| `SOUND_RECORDER_ANDROID_PLAY_STORE_URL` | unset | `/download/android` target. |
| `RUST_LOG` | `dd_sound_recorder_rs=info,tower_http=warn` | Filter for structured `tracing` logs. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset | OTLP/gRPC collector endpoint, for example `http://dd-otel-collector.observability.svc.cluster.local:4317`. |
| `OTEL_RESOURCE_ATTRIBUTES` | unset | Optional non-secret resource attributes; service identity cannot be overridden and secret-like keys are rejected. |

`/readyz` performs a live Postgres `select 1`, checks storage-history compatibility, and requires a
bounded remote `HeadObject` of `SOUND_RECORDER_S3_READINESS_OBJECT_KEY`. It deliberately does not
call `HeadBucket` or `ListBucket`: a least-privilege principal can access the configured object
prefix without a list grant. Signing-only readiness is available solely through
the explicit development flag above. Shared-auth and Supabase are not probed as
readiness dependencies: existing device-token traffic remains available during
a short auth-provider outage. If Supabase is explicitly required, readiness
also fetches and parses a non-empty JWKS for asymmetric projects or calls the
documented Auth health route for legacy HS256 projects. Durable token pepper,
registration posture, and internal auth remain required. `/healthz` reports
process health, the non-secret storage fingerprint, auth posture, and
configuration booleans without contacting dependencies. Unknown boolean
spellings are invalid configuration and fail readiness; they never silently
become `false`.

## Observability

Every HTTP request creates an OpenTelemetry-compatible span and records
low-cardinality method, route-template, status, and duration metrics. Logs are
newline-delimited JSON on stderr so Kubernetes Promtail can forward them to
Loki. When `OTEL_EXPORTER_OTLP_ENDPOINT` is configured, traces and metrics are
also exported to the cluster collector for its Prometheus/Tempo pipeline. The
existing `/metrics` Prometheus exposition remains available for direct scrape
compatibility.

## CLI flags

[`flags-2-env`](https://github.com/ORESoftware/flags-2-env) maps the declared
options in `.cli-flags.toml` onto the existing environment contract before the
backend starts:

```sh
scripts/with-flags help
scripts/with-flags audit
scripts/with-flags --port=8126 --supabase-url=https://project.supabase.co -- cargo run
```

The wrapper uses the monorepo's pinned native source when available and builds
it into a commit-keyed user cache. Set `FLAGS2ENV_BIN` for a standalone install.
Database credentials, JWT secrets, service-role keys, token peppers, and server
auth secrets intentionally remain environment-only.

## AWS S3 and Cloudflare R2

The service supports either AWS S3 or Cloudflare R2 as its primary private object store through the
same S3 API contract. For R2, set `SOUND_RECORDER_R2_ACCOUNT_ID`, bucket, access-key id, and secret;
the endpoint and `auto` signing region are selected automatically. The generic
`R2_BUCKET`, `R2_REGION`, `R2_ENDPOINT`, `R2_ACCESS_KEY_ID`, and
`R2_SECRET_ACCESS_KEY` names emitted by the shared infrastructure templates are
also accepted. An explicit
`SOUND_RECORDER_S3_ENDPOINT=https://<ACCOUNT_ID>.r2.cloudflarestorage.com` works as
well. Presigned URLs use the S3 API domain, not an R2 custom domain. PUT URLs
sign the expected content type and, when supplied, byte length; completion
verifies the stored object before it becomes visible in the timeline or
cloud-copy queue.

Production readiness uses a persistent sentinel inside `SOUND_RECORDER_S3_KEY_PREFIX`, proving
remote endpoint reachability and object authorization without bucket listing. Deployment validation
should additionally use a throwaway key and exercise PUT -> HEAD -> GET -> DELETE. The runtime
principal needs only object-level write/read/delete permissions on that prefix.

AWS/custom S3 buckets must be unversioned. In a versioned or versioning-suspended bucket,
`DeleteObject`/`DeleteObjects` against a key leave older versions behind, so retention and account
deletion would not be physical erasure. Supporting such buckets requires durable storage of version
IDs plus list-and-delete-all-versions work; the current schema and workers do not provide that.

This is a primary-backend choice, not an untracked simultaneous mirror. A durable AWS S3 -> R2
mirror needs a schema-owned object-copy job (source/destination provider, bucket/key, status,
attempts, lease, last error, completion timestamp), relaxed `storage_provider` constraints, a
retrying worker, and retention/account-deletion coordination. The existing user cloud-copy table is
constrained to Google Drive, OneDrive, and iCloud and cannot safely represent R2 jobs. Adding a
best-effort copy without that durable state would falsely report protection after a lost write, so
the backend deliberately does not do that.

Likewise, changing the single global backend/endpoint is not an implicit cutover. New upload-session
and segment metadata carries `sonusAurisStorageFingerprint`, derived from backend kind, endpoint,
region, and bucket. Readiness rejects marked rows from any other fingerprint and, by default,
pre-fingerprint rows too. During rollout only, an operator may set
`SOUND_RECORDER_ALLOW_UNMARKED_STORAGE_HISTORY=true` after proving all unmarked keys belong to the
currently configured store, and set `SOUND_RECORDER_UNMARKED_STORAGE_HISTORY_FINGERPRINT` to the
exact fingerprint exposed by `/healthz`. This binding automatically fails if the backend changes.
Use that value to backfill both segment and upload-session metadata, then disable the acknowledgment.
Before a later AWS/R2 switch, migrate or delete every historical object and update its durable
ownership metadata; otherwise readiness stays closed. True simultaneous/migrating providers require
schema-owned provider identity, per-provider clients, and durable copy/delete jobs rather than the
current single client.

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
SOUND_RECORDER_REQUIRE_SUPABASE=false \
SOUND_RECORDER_DEVICE_TOKEN_PEPPER=local-dev-pepper \
SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= \
SOUND_RECORDER_SERVER_AUTH_SECRET=local-dev-secret \
cargo run
```

The page, health, metrics, and generated docs render without cloud credentials; strict `/readyz`
correctly remains unavailable until Postgres and object storage are reachable. Mobile write paths
need the Postgres tables (schema lives in the `ores/k8s-cluster` monorepo under
`remote/libs/pg-defs/schema/schema.sql`) plus S3 credentials. The `migrations/` directory here is
applied out-of-band — see `migrations/RUNBOOK.md`.
