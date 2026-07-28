# Executable HTTP API contract standard

## Invariant

For every HTTP server, live route registration, request DTOs, response DTOs,
error DTOs, OpenAPI operations, runtime documentation, committed contracts, and
generated SDKs must originate from the same executable declaration. A second
hand-written route table, copied schema, or manually maintained SDK URL is not
an authoritative source of truth.

## Rust baseline

Migrated Axum services use pinned Utoipa 5.5 and `utoipa-axum` 0.2. Each handler
is annotated with `#[utoipa::path]` and registered through
`OpenApiRouter::routes(routes!(handler))`. That registration creates both the
Axum route and the OpenAPI path. Request, success-response, and error schemas
derive from the actual Serde wire DTOs. Stable, unique `operationId` values are
mandatory because generated client method names depend on them.

The exporter executes before environment parsing, telemetry initialization,
database connections, migrations, or dependency probes. CI can therefore run:

```bash
cargo run --locked --manifest-path <service>/Cargo.toml -- --export-openapi
```

without service credentials or live infrastructure.

## Standard runtime routes

Migrated HTTP services expose:

| Route | Contract |
| --- | --- |
| `GET /openapi.json` | Fail-closed public OpenAPI 3.1 document. |
| `GET /api/docs.json` | Compatibility alias serving the exact same public bytes. |
| `GET /api/docs` | Interactive Scalar reference for the public contract. |
| `GET /docs/api` | Compatibility alias for the same public reference. |

Private services additionally expose authenticated internal documentation routes,
conventionally `GET /internal/openapi.json` and `GET /internal/docs/api`. Those
routes serve the complete typed contract used by trusted service-to-service SDKs.

The internal OpenAPI document is generated once from the executable router and
canonicalized. The public artifact is a deterministic fail-closed projection of
that exact document. Standard runtime routes embed and serve the committed public
artifact byte-for-byte; internal routes serve the canonical full document. CI
rejects exporter drift, projection drift, runtime-byte drift, or a public artifact
containing private operations or schemas.

## Public and internal contracts

Every service is classified in `remote/api-contracts/manifest.json`. Public and
internal contracts are separate artifacts whenever the complete contract would
expose private routes or schemas. Public SDKs may only be generated from an
explicit public artifact; internal automation uses the internal artifact.
Filtering is allowlist-based, and publication is blocked until every operation
has an explicit visibility classification.

`dd-embeddings-rs` is the first native private-contract migration. Its contract
is suitable for private service-to-service SDKs; public package publication
remains blocked until gateway exposure and tenant authorization are explicit.

## Drift gates

`remote/tools/check-openapi-contracts.mjs` enforces:

1. two exporter runs are byte-identical;
2. exported bytes equal the committed contract;
3. OpenAPI is 3.1.x and includes title/version metadata;
4. operation IDs are present and unique;
5. every operation declares responses;
6. body methods use typed request bodies;
7. functional operations declare security; and
8. all standard documentation routes are present; and
9. every local `$ref` resolves inside the committed document.

The fleet compatibility inventory consumes a migrated service's committed
OpenAPI artifact. Regex source scanning remains a temporary, explicitly
allowlisted bridge only for services not yet migrated.

## SDK generation

`remote/tools/generate-openapi-sdks.mjs` validates the exact committed contract
and invokes a pinned OpenAPI Generator container for Rust, TypeScript Fetch, and
Dart clients. CI compiles or type-checks every generated smoke tree. Published
clients belong in the canonical private/public library repository
`ORESoftware/k8s-libs-and-shared-defs`, where generated output is committed with
a no-diff regeneration gate and OpenAPI digest provenance.

Generated clients are replaceable build output. Hand-written convenience
facades may wrap them, but may not duplicate route strings or wire models.

## Rollout

1. Inventory each deployment as HTTP, protocol-only, worker, or non-server.
2. Migrate local Rust/Axum services to the executable Utoipa pattern.
3. Migrate Node/Fastify services to route schemas consumed by runtime validation,
   TypeScript inference, and `@fastify/swagger`.
4. Migrate Gleam and Dart servers to typed route registries shared by dispatch,
   OpenAPI rendering, and client generation.
5. Update deployment gitlink source repositories directly, then bump the parent
   pointers in `k8s-cluster`.
6. Publish versioned public/internal SDKs only after compatibility review and
   digest-provenance checks pass.
