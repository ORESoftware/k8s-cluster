# Browser test server

`browser-test-server` runs bounded Playwright, Puppeteer, or Selenium scenarios for trusted cluster callers. It is packaged and deployed as `dd-browser-test-server`, separately from `dd-web-scraper`, while reusing the same hardened browser runtime image.

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

## Driver selection

- **Playwright** is the default for most UI, discovery, and verification flows.
- **Puppeteer** is for Chromium/CDP-specific integrations or existing Puppeteer adapters.
- **Selenium** is for WebDriver compatibility and the dedicated Selenium/Grid lane.

Do not run all three drivers for every job. A fallback driver is appropriate only for a classified rendering or driver-compatibility failure. Authentication failures, CAPTCHAs, robots/terms restrictions, and source-policy failures remain terminal or manual-review regardless of driver.

For the Benefactor prospecting integration, including job construction, source policy, provenance, deduplication, engine fallback, artifact handling, HubSpot/Postgres synchronization, and separation from Gmail/SendGrid delivery, see [`docs/benefactor-node-browser-automation.md`](../../../docs/benefactor-node-browser-automation.md).

## Driver selection

- **Playwright** is the default for most UI, discovery, and verification flows.
- **Puppeteer** is for Chromium/CDP-specific integrations or existing Puppeteer adapters.
- **Selenium** is for WebDriver compatibility and the dedicated Selenium/Grid lane.

Do not run all three drivers for every job. A fallback driver is appropriate only for a classified rendering or driver-compatibility failure. Authentication failures, CAPTCHAs, robots/terms restrictions, and source-policy failures remain terminal or manual-review regardless of driver.

For the Benefactor prospecting integration, including job construction, source policy, provenance, deduplication, engine fallback, artifact handling, HubSpot/Postgres synchronization, and separation from Gmail/SendGrid delivery, see [`docs/benefactor-node-browser-automation.md`](../../../docs/benefactor-node-browser-automation.md).

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

## Security defaults

Scenario execution requires the shared `SERVER_AUTH_SECRET` presented through the internal gateway. Arbitrary JavaScript evaluation is fail-closed: `BROWSER_TEST_ALLOW_EVALUATE` defaults to `false` and must be enabled explicitly only for a bounded trusted workflow. The deployment also caps concurrent scenarios, step count, scenario timeout, and screenshot size.

Keep caller-specific selectors, permitted source URLs, browser sessions, and business policy outside the shared runtime. Do not log page text, cookies, authorization headers, contact data, screenshots, or browser storage. The scheduler or agent that requests work must use a connected control-plane/queue rather than receiving cluster or provider credentials directly.

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
