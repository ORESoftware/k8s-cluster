# Browser test server

`browser-test-server` runs bounded Playwright, Puppeteer, or Selenium scenarios for trusted cluster callers.

## Executable API contract

The service uses one typed route registry and one set of Zod wire schemas for:

- Fastify handler dispatch;
- runtime request validation and response serialization;
- deterministic OpenAPI 3.1 export;
- fail-closed public/internal contract filtering; and
- generated Rust, TypeScript, Dart, and fleet SDK inputs.

Fastify receives the exact JSON Schema produced from the Zod models. The OpenAPI exporter applies only generator-compatibility normalization, preserving constant semantics through vendor metadata while avoiding language-generator bugs. A route or wire model must not be added through a second documentation-only definition.

## Documentation routes

Public, unauthenticated routes expose only explicitly public operations:

- `GET /openapi.json`
- `GET /api/docs.json`
- `GET /docs/api`
- `GET /api/docs`

The complete internal contract is available only to authenticated service callers:

- `GET /internal/openapi.json`
- `GET /internal/docs/api`

`POST /run`, tool inventory, status, and compatibility aliases remain internal. Kubernetes liveness and Prometheus metrics retain their intended public operational routes.

## Local contract checks

```bash
pnpm install --frozen-lockfile --ignore-workspace
pnpm run typecheck
pnpm run build
pnpm run test:contract
pnpm run export:openapi > generated/openapi.json
```

CI additionally exports twice and compares bytes, verifies runtime route/OpenAPI parity, regenerates compatibility artifacts and public/private fleet SDKs, compiles generated Rust and TypeScript clients, analyzes the generated Dart client, and rejects stale generated output.

Contract export constructs the Fastify application without binding a socket, starting telemetry exporters, or launching a browser.
