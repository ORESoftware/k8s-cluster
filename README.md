# ClipTown

ClipTown is an open-source, cross-platform clipboard history that makes copied text, links, images, and files searchable, pinnable, and available across trusted devices.

## Repository map

- [`cliptown-rust-backend.rs`](https://github.com/cliptown/cliptown-rust-backend.rs) — Rust API using SeaORM and PostgreSQL.
- [`cliptown-flutter`](https://github.com/cliptown/cliptown-flutter) — Flutter desktop and mobile applications.
- [`cliptown-clients`](https://github.com/cliptown/cliptown-clients) — Rust, TypeScript, and Dart SDKs.
- [`cliptown-interfaces`](https://github.com/cliptown/cliptown-interfaces) — versioned OpenAPI, Protobuf, JSON Schema, and language models.
- [`cliptown-infra`](https://github.com/cliptown/cliptown-infra) — Argo CD and Kubernetes deployment definitions.
- [`cliptown-cli`](https://github.com/cliptown/cliptown-cli) — Rust command-line client using `flags-2-env`.
- [`cliptown-extension`](https://github.com/cliptown/cliptown-extension) — consent-based browser capture and draft recovery.
- [`cliptown.github.io`](https://github.com/cliptown/cliptown.github.io) — public product and security site.

The repositories above are included under `apps/` as secondary submodule checkouts. Their standalone repositories and `main` branches remain the source of truth.

## Companion applications

[Memebank](https://github.com/memebank) is a companion image and meme catalog. MemeBank owns ingestion, OCR, visual tagging, semantic retrieval, collections, and sharing policy; ClipTown owns generic clipboard history and encrypted transfer synchronization.

MemeBank interoperability is API- and SDK-only. MemeBank obtains a short-lived, audience- and scope-limited ClipTown token from shared-auth and calls the versioned ClipTown transfer API through an official SDK. The integration never requires both mobile apps to be installed, never probes app presence, and never uses deep links, local IPC, loopback services, clipboard monitoring, shared databases, or shared cloud credentials as product-to-product transport. Native clipboard export remains a separate user feature.

See [`docs/memebank-integration.md`](docs/memebank-integration.md) for the authentication, authorization, ciphertext, idempotency, availability, and conformance contract.

## Authentication

Supabase may remain part of ClipTown's primary identity and session implementation, and a six-digit PIN or platform biometric may protect local device key material. For MemeBank interoperability, however, shared-auth is the sole cross-product authentication and assurance boundary. Any assurance produced through 3FA, passkeys, TOTP, email OTP, SMS OTP, or recovery is consumed only as signed shared-auth `aal`/`acr`/`amr`/`auth_time` claims—not as a direct factor-app token or proof.

Shared-auth delegates narrowly scoped tokens with audience `cliptown-api`, authorized party `memebank-api`, and one `cliptown:memebank:*` scope. ClipTown verifies the delegated token and enforces subject/resource ownership on every request. Clients must not infer authorization from local state or from whether another application is installed.

The default reauthentication interval is 10 days. A user may configure an interval up to 20 days. Device and session revocation must invalidate subsequent sync operations; sensitive MemeBank write and delete scopes additionally require recent LOA2 under shared-auth policy.

## End-device encryption

Clipboard content is encrypted on a trusted device before cloud or peer synchronization.

- A cryptographically random account master key is generated on a trusted device.
- The six-digit PIN is **not** the clipboard encryption key. A memory-hard KDF may derive a local key-encryption key that unwraps protected key material.
- Raw fingerprint, face, thumbprint, or voice-recognition data is never stored in ClipTown or used directly as encryption-key material.
- Clip envelopes use authenticated encryption such as XChaCha20-Poly1305 or AES-256-GCM with associated-data binding.
- The Rust backend stores and transports ciphertext, integrity metadata, opaque search artifacts, and explicitly opted-in embeddings; it does not require plaintext clipboard content.
- Device enrollment, recovery, rotation, revocation, and peer pairing must be authenticated and auditable.

The canonical wire and security invariants live in `cliptown-interfaces`. Material security changes must update that repository before dependent implementations.

## Reproducible development

The first shared local toolchain contract is in [`mise.toml`](mise.toml), with the compatibility and platform matrix documented in [`docs/toolchain.md`](docs/toolchain.md).

```sh
bash scripts/bootstrap.sh
bash scripts/validate-workspace.sh
```

These commands initialize reviewed submodules and run non-production validation without requesting signing identities, production secrets, cloud credentials, or cluster access.

## Development workflow

1. Cut feature branches from `main` in the standalone repository.
2. Run that repository's complete CI matrix.
3. Merge the standalone PR.
4. Update this monorepo's submodule pointer in a separate PR.
5. Update the `ORESoftware/k8s-cluster` pointer only after the monorepo commit is merged.
