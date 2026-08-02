# Browser test server

`dd-browser-test-server` runs bounded Playwright, Puppeteer, or Selenium scenarios for trusted cluster callers.

It is a separate API and Kubernetes service from `dd-web-scraper`, while reusing the same hardened browser-capable image family and Chromium runtime conventions.

## Security boundary

Internal endpoints, including `POST /run`, require the opaque `SERVER_AUTH_SECRET` through `X-Server-Auth`, an `Authorization: Bearer` header, or the legacy `X-Auth` header. Kubernetes obtains the secret from `dd-agent-secrets`; the value must never be committed or written to browser evidence.

The declarative scenario DSL is the default execution surface. Arbitrary page evaluation remains disabled unless an operator explicitly sets `BROWSER_TEST_ALLOW_EVALUATE=true`. Production manifests set `BROWSER_TEST_ALLOW_EVALUATE=false`, and callers should prefer typed `fill`, `select`, `click`, wait, extraction, and screenshot steps.

## Executable API contract

The service uses one typed route registry and one set of Zod wire schemas for:

- Fastify handler dispatch;
- authoritative runtime request validation;
- Fastify response serialization;
- deterministic OpenAPI 3.1 export;
- fail-closed public/internal contract filtering; and
- generated Rust, TypeScript, Dart, and fleet SDK inputs.

Zod is the single authoritative request-body validator and normalizer. Fastify parses JSON, enforces body limits, executes the Zod `preValidation` boundary, and serializes responses from generated response schemas. Request JSON Schema is still generated from the same Zod models for OpenAPI documentation and SDK generation, but it is not re-applied through Fastify AJV: AJV's default `removeAdditional` behavior can mutate a discriminated-union object while evaluating an earlier `oneOf` branch before its matching branch is considered.

The OpenAPI exporter applies only generator-compatibility normalization, preserving constant semantics through vendor metadata while avoiding language-generator bugs. A route or wire model must not be added through a second documentation-only definition.

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

A real-driver smoke can be run after installing Playwright Chromium and setting the selected driver:

```bash
pnpm exec playwright install --with-deps chromium
BROWSER_TEST_CHROMIUM_PATH="$(node --input-type=module -e "import { chromium } from 'playwright'; process.stdout.write(chromium.executablePath())")" \
BROWSER_TEST_TOOL=playwright \
node scripts/browser-driver-smoke.mjs
```

GitHub Actions runs the same production `dist/server.js` and authenticated loopback scenario independently with Playwright, Puppeteer, and Selenium. It retains sanitized result metadata and bounded service logs for each driver.

CI additionally exports twice and compares bytes, verifies runtime route/OpenAPI parity, regenerates compatibility artifacts and public/private fleet SDKs, compiles generated Rust and TypeScript clients, analyzes the generated Dart client, and rejects stale generated output.

Contract export constructs the Fastify application without binding a socket, starting telemetry exporters, or launching a browser.
