# Executable HTTP API contract

The running Axum routes and the OpenAPI operations are registered together through `utoipa_axum::routes!`. `generated/openapi.internal.json` is the complete private SDK source. `generated/openapi.public.json` is a fail-closed projection containing only health, readiness, and standard documentation routes.

Standard routes:

- `GET /openapi.json` and `GET /api/docs.json`: public OpenAPI 3.1 JSON;
- `GET /api/docs` and `GET /docs/api`: public Scalar reference;
- `GET /internal/openapi.json`: authenticated private OpenAPI JSON;
- `GET /internal/docs/api`: authenticated private Scalar reference.

Export without runtime credentials or network access:

```bash
cargo run --locked -- --export-openapi=public
cargo run --locked -- --export-openapi=internal
```
