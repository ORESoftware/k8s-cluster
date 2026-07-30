# HTTP API, OpenAPI, and SDK Contract

This contract applies to every HTTP server under `remote/deployments`, including services
implemented in Rust, Node.js/TypeScript, Gleam, Dart, Python, Java, and F#.

## Non-negotiable invariants

1. **One typed source of truth.** The same declaration that registers the runtime route must
   supply its method, path, request validation, response schema, auth posture, visibility, and
   operation identifier to OpenAPI generation. A separately maintained YAML file, route list, or
   SDK model is not authoritative.
2. **OpenAPI 3.1 at runtime.** Every service exposes:
   - `GET /docs/api` — human-readable API reference.
   - `GET /api/docs` — compatibility alias for the human-readable reference.
   - `GET /api/docs.json` — fail-closed public OpenAPI 3.1 JSON for that running build. The full internal contract is never served by these public routes.
3. **Fail-closed public contracts.** An operation is internal unless its source declaration
   explicitly marks it public. The generated `api-docs.public.json` file contains only operations
   with `x-dd-visibility: public`; private and operator routes must never enter public SDKs.
4. **Generated SDKs are build products.** Public and internal SDKs are generated only from the
   matching OpenAPI artifact. CI regenerates the spec and clients, compiles them, and rejects any
   diff. Hand-edited generated clients are not accepted.
5. **Runtime/spec parity is tested.** Contract tests compare the registered method/path set with
   OpenAPI, verify unique `operationId` values, exercise the docs routes, and run representative
   generated-client calls against an in-process server or ephemeral test server.

## Canonical generated artifacts

For the default output name `api-docs`, each service owns:

| Artifact | Purpose |
| --- | --- |
| `generated/api-docs.json` | Fail-closed public OpenAPI 3.1 document served at `/api/docs.json` and used for public SDKs. |
| `generated/api-docs.html` | Public-only human-readable reference served by the two HTML routes. |
| `generated/api-docs.internal.json` | Full, unserved contract used only for private SDKs and CI parity checks. |
| `generated/api-docs.metadata.json` | Migration/debug metadata about discovered source routes; not a consumer contract. |

The current fleet generator produces route-complete OpenAPI documents from existing route
registrations and preserves richer source metadata as a companion artifact. This is a migration
bridge, not the end state: request/response schemas become authoritative only when a service moves
to its native typed adapter.

## Native source-of-truth strategies

### Rust and Axum

Use `utoipa` schemas and `#[utoipa::path]` on the handler, then register the handler through
`utoipa_axum::routes!` and `OpenApiRouter`. Do not register a documented handler again with a
separate `Router::route` call. DTOs used on the wire derive `Serialize`/`Deserialize` and
`utoipa::ToSchema` from the same Rust type.

Target shape:

```rust
#[derive(serde::Deserialize, utoipa::ToSchema)]
struct CreateWidgetRequest {
    name: String,
}

#[utoipa::path(
    post,
    path = "/v1/widgets",
    request_body = CreateWidgetRequest,
    responses((status = 201, body = WidgetResponse)),
    tag = "widgets"
)]
async fn create_widget(/* extractors */) -> impl axum::response::IntoResponse {
    // implementation
}

let (router, openapi) = utoipa_axum::router::OpenApiRouter::new()
    .routes(utoipa_axum::routes!(create_widget))
    .split_for_parts();
```

Shared/private routes may be mounted outside the public router, but they still need an internal
contract if another service calls them.

### Node.js/TypeScript and Fastify

Declare each route with a Fastify route options object. Its schema is used simultaneously for
runtime validation, TypeScript inference, and `@fastify/swagger` generation. Prefer a supported
Fastify type provider (TypeBox or the maintained Zod provider) and do not keep separate interface,
validator, and OpenAPI definitions.

```ts
server.post(
  '/v1/widgets',
  {
    schema: {
      body: CreateWidgetSchema,
      response: { 201: WidgetSchema },
      tags: ['widgets'],
      operationId: 'createWidget',
      'x-dd-visibility': 'public',
    },
  },
  createWidget,
);
```

The Swagger plugin must be registered before routes, and the exported document must be obtained
from the initialized Fastify instance so plugins and route prefixes are represented exactly.

### Gleam

Use a typed route registry whose entries contain the method, path template, operation ID,
visibility, schemas, and a typed route identifier. The same registry must:

1. match the incoming `mist`/`wisp` request and return the typed route identifier;
2. drive the handler dispatch `case`;
3. render OpenAPI; and
4. feed client generation.

Do not add a new route only to a hand-written `case req.method, path_segments` expression.
`nori` may consume the resulting OpenAPI for generated Gleam models/clients, but the service's
typed route registry remains the runtime source of truth.

### Dart

Raw `HttpServer` deployments use a typed `ApiRoute` registry or generated router. The registry
owns method, path template, operation ID, visibility, request/response JSON Schema, and handler.
Both request dispatch and OpenAPI rendering iterate that registry. Long `if (method == ... &&
path == ...)` chains are migration-only and may not be extended without adding the corresponding
typed route entry and parity test.

### Other runtimes

- Python: prefer FastAPI/Pydantic route declarations and its runtime OpenAPI export.
- Java/Scala: use a typed Vert.x/Spring route descriptor that supplies both routing and OpenAPI.
- F#: use a typed endpoint descriptor consumed by both the Giraffe/Saturn router and OpenAPI
  renderer.

A framework-specific implementation is acceptable only when it preserves the five invariants
above.

## Public and internal SDK synchronization

Each service declares operation visibility in source:

- `public`: included in full and public OpenAPI; eligible for externally published SDKs.
- `internal`: included only in the full/internal contract; eligible for private workspace SDKs.
- `none`: operational endpoints such as probes may appear in docs but are excluded from SDK
  generation.

The generation pipeline is:

```text
typed route/schema declaration
        ↓
running router + runtime validation
        ↓
OpenAPI 3.1 export
        ↓
public/internal filtering
        ↓
language SDK generation
        ↓
format + compile + contract tests
        ↓
publish with OpenAPI SHA-256 provenance
```

SDK packages must record the source service, service version/commit, OpenAPI digest, and generator
version. Publishing is refused when the committed OpenAPI or generated SDK tree differs from a
fresh generation.

The default client targets are:

- Rust: `reqwest`-based async client crate.
- TypeScript: standards-based `fetch` client with exported request/response types.
- Dart: `http`/`dio` client package suitable for Flutter and server-side Dart.
- Gleam: typed client using the selected HTTP client package.

Public packages are generated from the runtime-safe `api-docs.json`; workspace/private packages are generated
from the unserved `api-docs.internal.json`. A public package must never import or expose an internal operation.

## CI gates

`remote/tools/generate-api-docs.mjs --check` verifies deterministic generated files.
`remote/tools/validate-openapi-contracts.mjs` additionally verifies:

- every indexed available service has public runtime, public HTML, internal, and metadata artifacts;
- full documents are OpenAPI 3.1;
- registered method/path pairs and OpenAPI operations are identical;
- every operation has a fleet-unique `operationId`;
- visibility and auth metadata are present;
- the runtime document is the exact public subset of the unserved internal document; and
- all three standard docs routes are documented.

The explicit scanner allowlist in `remote/config/api-contracts.json` prevents accidental expansion
of the migration bridge. A new service must choose a native typed strategy instead of silently
falling back to regex scanning.

## Migration sequence

1. Keep the fleet-wide OpenAPI bridge and parity gates green.
2. Migrate one service at a time to its native typed strategy.
3. Add request/response schemas and explicit operation visibility.
4. Generate and compile public/internal SDKs.
5. Remove the service from `legacySourceScannerAllowlist`.
6. Delete scanner support for a runtime after its final service migrates.

A migration is complete only when route registration, runtime validation, OpenAPI, generated SDKs,
and contract tests all originate from the same typed declaration.
