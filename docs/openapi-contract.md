# Executable HTTP API contract

The running Axum routes and the OpenAPI operations are registered together through `utoipa_axum::routes!`. `generated/openapi.internal.json` is the complete private SDK source. `generated/openapi.public.json` is a fail-closed projection containing only health, readiness, and standard documentation routes.

Standard routes:

- `GET /openapi.json` and `GET /api/docs.json`: public OpenAPI 3.1 JSON;
- `GET /api/docs` and `GET /docs/api`: public Scalar reference;
- `GET /internal/openapi.json`: authenticated private OpenAPI JSON;
- `GET /internal/docs/api`: authenticated private Scalar reference.

## Public exposure invariant

The public document is not only path-filtered. Its component graph is rebuilt transitively from schemas referenced by public operations. Private delivery types—including push jobs, contact jobs, provider payloads, device capabilities, and recipient targets—must therefore be absent from the public document even when they remain present in the internal SDK contract. Both documents carry an explicit `x-dd-contract-scope` marker, and service-owned metadata replaces dependency-library author and license metadata.

## HTTP boundary invariant

The combined application router preserves its route-family limits after the push, contact, and documentation routers are merged:

- push mutation bodies: at most 512 KiB;
- contact mutation bodies: at most 768 KiB;
- protected operations authenticate before validating or dispatching payloads;
- readiness fails closed when authentication or providers are unavailable;
- error bodies and logs must not echo bearer secrets, device capabilities, or recipient data.

These invariants are exercised by Rust integration tests and Chromium/API smoke tests in GitHub Actions. The workflows also compare two independent exports per scope, compare them with committed artifacts, and reject path, schema, security, operation-ID, formatting, lint, and repository-hygiene drift.

Export without runtime credentials or network access:

```bash
cargo run --locked -- --export-openapi=public
cargo run --locked -- --export-openapi=internal
```
