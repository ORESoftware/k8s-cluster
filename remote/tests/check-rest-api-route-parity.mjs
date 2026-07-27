#!/usr/bin/env node
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

function findRepoRoot() {
  for (const candidate of [process.cwd(), resolve(__dirname, '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/tools/generate-api-docs.mjs'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

function runNode(repoRoot, script, args = []) {
  const result = spawnSync('node', [script, ...args], {
    cwd: repoRoot,
    encoding: 'utf8',
    timeout: 60_000,
  });
  assert.equal(
    result.status,
    0,
    `${script} ${args.join(' ')} failed.\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`,
  );
  return result.stdout.trim();
}

function readJson(repoRoot, path) {
  return JSON.parse(readFileSync(resolve(repoRoot, path), 'utf8'));
}

function canonicalOpenApiPath(service) {
  return service.generated.find(
    (path) => path.endsWith('.json') && !path.endsWith('.public.json') && !path.endsWith('.metadata.json'),
  );
}

const repoRoot = findRepoRoot();
const generationOutput = runNode(repoRoot, 'remote/tools/generate-api-docs.mjs', ['--check']);
const validationOutput = runNode(repoRoot, 'remote/tools/validate-openapi-contracts.mjs');

const restApiOpenApiPath = 'remote/deployments/rest-api-rs/generated/api-docs.json';
const restApiMetadataPath = 'remote/deployments/rest-api-rs/generated/api-docs.metadata.json';
const restApiPublicPath = 'remote/deployments/rest-api-rs/generated/api-docs.public.json';
const restApiOpenApi = readJson(repoRoot, restApiOpenApiPath);
const restApiMetadata = readJson(repoRoot, restApiMetadataPath);
const restApiPublic = readJson(repoRoot, restApiPublicPath);

assert.equal(restApiOpenApi.openapi, '3.1.0');
assert.equal(restApiOpenApi['x-dd-contract-scope'], 'internal');
assert.equal(restApiPublic['x-dd-contract-scope'], 'public');
assert.equal(restApiMetadata.routeTypeCounts['user-generated'], 26);
assert.ok(!Object.prototype.hasOwnProperty.call(restApiMetadata.routeTypeCounts, 'pg' + '-first'));
for (const path of ['/docs/api', '/api/docs', '/api/docs.json']) {
  assert.ok(
    restApiMetadata.routes.some((route) => route.path === path && route.methods.includes('GET')),
    `rest-api-rs generated metadata is missing GET ${path}`,
  );
  assert.ok(restApiOpenApi.paths[path]?.get, `rest-api-rs OpenAPI is missing GET ${path}`);
  assert.ok(restApiPublic.paths[path]?.get, `rest-api-rs public OpenAPI is missing GET ${path}`);
}
assert.ok(
  restApiMetadata.routes.every((route) => !route.path.startsWith('/api/db')),
  'rest-api-rs generated metadata must not expose generic /api/db routes.',
);
assert.ok(
  Object.keys(restApiOpenApi.paths).every((path) => !path.startsWith('/api/db')),
  'rest-api-rs OpenAPI must not expose generic /api/db routes.',
);
assert.ok(
  readFileSync(resolve(repoRoot, 'remote/deployments/rest-api-rs/src/main.rs'), 'utf8').includes(
    'router.nest("/internal/db", db_routes::router())',
  ),
  'generic DB inspection routes, if kept, must live under /internal/db and be explicitly gated.',
);

const index = readJson(repoRoot, 'remote/deployments/generated-api-docs-index.json');
assert.ok(index.services.length >= 48, 'expected generated API contracts for the HTTP service fleet');
assert.deepEqual(index.centralDocsRoutes, ['/api-docs', '/api-docs.json']);
assert.deepEqual(index.standardDocsRoutes, ['/docs/api', '/api/docs', '/api/docs.json']);
for (const serviceName of ['dart-server', 'fsharp-ws-server']) {
  assert.ok(
    index.services.some((service) => service.service === serviceName),
    `${serviceName} must stay inside generated API contract coverage`,
  );
}
assert.ok(
  readFileSync(resolve(repoRoot, 'remote/deployments/generated-api-docs-index.html'), 'utf8').includes(
    'dd runtime API docs',
  ),
  'central generated API docs HTML index must be committed and servable by web-home-rs.',
);

for (const service of index.services) {
  const openApiPath = canonicalOpenApiPath(service);
  assert.ok(openApiPath, `${service.service} must include a canonical OpenAPI JSON artifact`);
  const publicPath = openApiPath.replace(/\.json$/, '.public.json');
  const metadataPath = openApiPath.replace(/\.json$/, '.metadata.json');
  for (const path of [openApiPath, publicPath, metadataPath]) {
    assert.ok(existsSync(resolve(repoRoot, path)), `${service.service} is missing ${path}`);
  }

  const openapi = readJson(repoRoot, openApiPath);
  const publicOpenApi = readJson(repoRoot, publicPath);
  const metadata = readJson(repoRoot, metadataPath);
  assert.equal(openapi.openapi, '3.1.0', `${service.service} must emit OpenAPI 3.1`);
  assert.equal(openapi['x-dd-service'], service.service);
  assert.equal(publicOpenApi['x-dd-contract-scope'], 'public');
  assert.equal(metadata.service, service.service);
  for (const path of metadata.standardDocsRoutes) {
    assert.ok(openapi.paths[path]?.get, `${service.service} OpenAPI is missing GET ${path}`);
    assert.ok(publicOpenApi.paths[path]?.get, `${service.service} public OpenAPI is missing GET ${path}`);
  }
}

console.log([generationOutput, validationOutput].filter(Boolean).join('\n'));
