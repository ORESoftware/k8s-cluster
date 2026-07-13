# `src/` — service source

The entire `dd-sound-recorder-rs` binary lives here. There is exactly one file.

## Files

- **`main.rs`** — the whole Axum/Tokio service in one file (~6.9k lines). It is intentionally
  *not* split into modules; the top-of-file `//!` module doc comment is the map. In order, it
  contains:
  - Prometheus metric collectors and service constants/limits.
  - Configuration loading (`Config`, `SupabaseConfig`, `S3StorageConfig`, the `env_*` helpers,
    `config_from_env`, `state_from_config`) and the shared `AppState` (Postgres `bb8` pool, S3
    client, `CloudTokenSealer`).
  - `ServiceError` and the request/response DTO structs.
  - Auth: Supabase JWT verification with cached JWKS (`SupabaseVerifier`), opaque device bearer
    tokens (SHA-256 + pepper), the registration bearer, and the internal server-auth secret.
  - Route handlers — device registration, upload sessions & segment presign
    (`presign_put`/`presign_get`), timeline, evidence exports, permanent saves, alerts and the
    `/listen/:alert_id` page, cloud-connection OAuth linking, the cloud-copy drain worker
    (Google Drive / OneDrive upload), account deletion, and the retention sweep.
  - `rate_limit` and `add_security_headers` middleware.
  - `main()` (router wiring, TLS Postgres, graceful shutdown) and a trailing `#[cfg(test)]`
    unit-test module.

## Notes

- Audio bytes are never proxied through this process — it deals only in presigned S3 URLs and
  metadata rows.
- Postgres schema is owned out-of-band (see [`../migrations/README.md`](../migrations/README.md)),
  so there are no embedded migrations or ORM models here.
- See the repo [`readme.md`](../readme.md) for routes, environment variables, and deployment.
