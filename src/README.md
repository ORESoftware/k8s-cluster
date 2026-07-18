# `src/` — service source

The `dd-sound-recorder-rs` binary keeps its HTTP/business logic in `main.rs` and
its security-sensitive Supabase verification boundary in a focused module.

## Files

- **`main.rs`** — the Axum/Tokio service, routes, configuration, storage, and
  domain logic. Its top-of-file `//!` module doc comment is the map. In order,
  it contains:
  - Prometheus metric collectors and service constants/limits.
  - Configuration loading (`Config`, `SupabaseConfig`, `S3StorageConfig`, the `env_*` helpers,
    `config_from_env`, `state_from_config`) and the shared `AppState` (Postgres `bb8` pool, S3
    client, `CloudTokenSealer`).
  - `ServiceError` and the request/response DTO structs.
  - Auth: opaque device bearer tokens (SHA-256 + pepper), the registration
    bearer, and the internal server-auth secret.
  - Route handlers — device registration, upload sessions & segment presign
    (`presign_put`/`presign_get`), timeline, evidence exports, permanent saves, alerts and the
    `/listen/:alert_id` page, cloud-connection OAuth linking, the cloud-copy drain worker
    (Google Drive / OneDrive upload), account deletion, and the retention sweep.
  - `rate_limit` and `add_security_headers` middleware.
  - `main()` (router wiring, TLS Postgres, graceful shutdown) and a trailing `#[cfg(test)]`
    unit-test module.
- **`supabase_auth.rs`** — Supabase JWT verification and JWKS caching: explicit
  HS256/RS256/ES256 allowlist, issuer/audience/expiry checks, signing-key
  algorithm/use matching, bounded cache, and single-flight refresh throttling.

## Notes

- Normal mobile uploads/downloads never proxy audio through this process — it deals in presigned
  AWS S3 or Cloudflare R2 URLs and metadata rows. The server does use authenticated `HeadObject`
  to verify completion, and reads bytes only for explicit Google Drive / OneDrive copy jobs.
- Production readiness uses a configured object sentinel for a bounded remote `HeadObject`; the
  signing-only fallback is an explicit development opt-out. New rows also carry a backend
  fingerprint so changing the one global S3/R2 client cannot silently misroute historical keys.
- AWS/custom S3 must be explicitly unversioned. Key-only deletion cannot physically erase old
  versions from versioned or versioning-suspended buckets; R2 does not support versioning.
- Postgres schema is owned out-of-band (see [`../migrations/README.md`](../migrations/README.md)),
  so there are no embedded migrations or ORM models here.
- See the repo [`readme.md`](../readme.md) for routes, environment variables, and deployment.
