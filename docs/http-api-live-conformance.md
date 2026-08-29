# Live HTTP API contract conformance

The executable OpenAPI migration is incomplete if a committed contract and generated SDKs are
correct in Git but the running service serves different bytes. The manifest-driven live conformance
harness closes that deployment boundary without introducing another route inventory.

## Command

Run the service through its intended HTTP entry point, then execute:

```bash
node remote/tools/check-live-api-contract.mjs \
  --service dd-embeddings-rs \
  --base-url https://dd-embeddings.example.com
```

The service name must exist in `remote/api-contracts/manifest.json`. The command reads the matching
`publicContract` artifact and verifies the standard public documentation routes declared by that
same manifest entry.

Use `--json` for a machine-readable report containing the service, base URL, committed public
contract path, SHA-256 digest, response sizes, media types, and per-route digests:

```bash
node remote/tools/check-live-api-contract.mjs \
  --service browser-test-server \
  --base-url https://browser-test.example.com \
  --json
```

## Enforced invariants

For every native service the harness requires:

1. `docsRoutes` is exactly `/openapi.json`, `/api/docs.json`, `/api/docs`, and `/docs/api`.
2. The committed public artifact is OpenAPI 3.1, identifies the selected service, is marked
   `x-dd-contract-scope: public`, includes every standard route, and contains no `/internal/` path.
3. `GET /openapi.json` and `GET /api/docs.json` return HTTP 200 with a JSON media type and bytes that
   exactly equal the committed public artifact. Semantic similarity is not sufficient.
4. `GET /api/docs` and `GET /docs/api` return HTTP 200 HTML that bootstraps `/openapi.json` and does
   not reference an internal documentation route or `api-docs.internal.json`.
5. Redirects are rejected, each request has a bounded timeout, and every response body is size
   limited before it is buffered.

The report has no wall-clock timestamp or random value, so its identity is derived from the actual
service URL and contract bytes rather than the runner.

## GitHub Actions

`.github/workflows/openapi-live-conformance.yml` runs hermetic unit tests using a loopback fixture
server. The tests prove the success path and reject byte drift, public HTML that references an
internal contract, and redirects. The workflow also runs the normal generated-contract drift and
public/internal validation gates.

A maintainer may manually dispatch the workflow with a manifest service key and a public HTTPS base
URL. The workflow has read-only repository permissions, receives no deployment or registry secret,
and checks only the unauthenticated public documentation surface.

## Rollout policy

Add live conformance after a service has a native executable contract and before treating its
migration as operationally complete. The service-specific deployment workflow should:

1. build and deploy one immutable source revision;
2. run the existing native export, generated artifact, and SDK gates;
3. call this harness against that exact deployment revision;
4. record the image digest, source revision, public contract SHA-256, SDK release identity, and
   rollback revision together; and
5. promote only the revision whose live public bytes match the reviewed artifact.

This command intentionally does not start services, authenticate to private routes, mutate a
cluster, publish packages, or infer endpoint schemas from source text. Trusted internal-client smoke
calls and service-specific functional calls remain separate deployment gates; they must consume the
internal SDK generated from the same accepted source revision.
