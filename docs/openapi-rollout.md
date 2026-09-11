# t2v-v2t executable OpenAPI contract

## Source of truth

The API server registers runtime routes and OpenAPI operations through the same `utoipa_axum::routes!` declarations in `crates/api/src/lib.rs`. Each annotated function in `http_contract.rs` delegates directly to the established speech, translation, Vapi, history, metrics, and readiness handler. There is no independent route inventory used to generate documentation.

The export path runs before telemetry initialization, database connections, provider construction, environment-backed secrets, or socket binding:

```bash
cargo run --locked -p t2v-api -- --export-openapi=public
cargo run --locked -p t2v-api -- --export-openapi=internal
```

Two clean exports must be byte-identical and equal the committed artifacts under `generated/`.

## Standard routes

Public documentation and JSON aliases:

- `GET /openapi.json`
- `GET /api/docs.json`
- `GET /api/docs`
- `GET /docs/api`

The public contract includes service health/readiness plus unauthenticated speech-to-text, text-to-speech, translation, audio analysis, and speech-to-speech routes. It deliberately excludes metrics, Vapi callbacks, operator call control, history, internal documentation, and every security scheme.

Authenticated internal documentation:

- `GET /internal/openapi.json`
- `GET /internal/docs/api`

These routes use the same fail-closed `Authorization: Bearer <T2V_SERVER_AUTH_SECRET>` middleware as history and operator call-control routes. The internal contract also documents the Vapi callback header scheme without moving that established constant-time validation out of the webhook handler.

## Visibility and authentication

- Public speech and translation actions: unauthenticated, bounded by existing audio/JSON body limits and request timeout.
- Prometheus metrics: runtime behavior remains unchanged, but the route is omitted from the public third-party contract and retained in the internal operator contract.
- Vapi webhook: authenticated by `x-vapi-secret` inside the existing handler; it is internal/partner-facing.
- History, Vapi call control, and internal docs: protected by `server_auth`; missing server configuration fails with 503 and missing/wrong credentials fail with 401.

## Browser and contract validation

Chromium exercises both Scalar aliases, byte-identical public JSON aliases, exact public path minimization, authenticated internal documents, declared security schemes, runtime health/readiness, history bearer enforcement, and Vapi callback enforcement.

Rust/CI validation additionally requires:

- formatting and warnings-denied Clippy;
- all API unit and integration tests;
- deterministic public/internal serialization;
- OpenAPI 3.1 and JSON Schema 2020-12 metadata;
- unique stable operation IDs;
- exact public/internal path sets;
- absence of private security definitions from the public document;
- no merge-conflict markers or stale generated artifacts.

## SDK boundary

Public SDKs must be generated only from `generated/openapi.public.json`. Private/operator SDKs may be generated from `generated/openapi.internal.json` and must remain separately named and access-controlled. The generated clients must preserve operation IDs, request deadlines, body/content negotiation, bearer injection, Vapi secret injection, request IDs, redaction, and retry/idempotency semantics where those are explicitly defined by the service contract.
