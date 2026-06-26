<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/3fa-backend` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/3FA-app/3fa-backend.rs` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/3fa-backend`.
<!-- END k8s-cluster-submodule-notice --># 3FA — Sync Server (backend)

Zero-knowledge sync for the 3FA authenticator. The server stores only an opaque,
client-encrypted vault blob plus a version vector — it can never read your OTP
seeds or your password. Written in Rust (axum + sqlx/Postgres).

> One of three repos:
> - **`3fa-desktop.rs`** — desktop app (Rust + Slint)
> - **`3fa-backend.rs`** — this server (Rust + axum)
> - **`3fa-website`** — marketing/download site (Astro)
>
> The wire-protocol types live in [`src/protocol.rs`](src/protocol.rs), a copy
> kept byte-for-byte in sync with the frontend's copy (guarded by
> `PROTOCOL_VERSION`).

## Endpoints

| Method | Path                  | Auth | Purpose                              |
|--------|-----------------------|------|--------------------------------------|
| POST   | `/v1/register`        | —    | Create account + first device, returns token |
| POST   | `/v1/login`           | —    | Verify account, register a device, returns token |
| GET    | `/v1/vault`           | ✓    | Pull the sealed vault blob           |
| POST   | `/v1/vault`           | ✓    | Push a sealed vault blob (version-vector reconciled) |
| POST   | `/v1/devices/revoke`  | ✓    | Revoke a device's sync token         |
| GET    | `/healthz`            | —    | Liveness                             |

## Security model

- **Zero-knowledge vault.** Clients E2E-encrypt the whole vault before upload
  (`protocol::SealedBlob`); the DB stores ciphertext only.
- **Account auth.** Argon2id verifier + per-device bearer tokens (only the
  token's SHA-256 is stored; login is constant-work to avoid user enumeration).
  OPAQUE PAKE is the planned drop-in (`src/auth.rs`) so the password never
  reaches the server at all.
- **Sync.** Per-device version vectors give last-writer-wins-with-merge; a stale
  push gets a `Conflict` and must pull/merge/retry.

## Run locally

```bash
DATABASE_URL=postgres://user:pass@localhost/threefa cargo run
# runs migrations on startup, serves on :8080 (override with BIND_ADDR)
```

`sqlx` uses runtime (non-macro) queries, so **no live database is needed to
build** — only to run.

## Layout

```
src/app.rs        Router, handlers, app state
src/auth.rs       Argon2id verifier + bearer tokens (OPAQUE seam)
src/vault_blob.rs Sealed-blob store + version-vector reconciliation
src/devices.rs    Device registration / revocation
src/db.rs         Pool + migration runner
src/protocol.rs   Wire-protocol DTOs (duplicated with the frontend)
migrations/       sqlx Postgres migrations
deploy/           Dockerfile + k8s/ArgoCD manifests
```

## Deploy

To the ORES `k8s-cluster` as a git submodule — see
[`deploy/README.md`](deploy/README.md).

## License

MIT OR Apache-2.0
