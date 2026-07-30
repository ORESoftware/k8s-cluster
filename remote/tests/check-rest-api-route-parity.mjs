#!/usr/bin/env node
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

function findRepoRoot() {
  for (const candidate of [process.cwd(), resolve(__dirname, '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/tools/generate-api-docs.mjs'))) return candidate;
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

function deploymentGitlinks(repoRoot) {
  const output = execFileSync('git', ['ls-files', '--stage', '--', 'remote/deployments'], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  return new Set(
    output
      .split('\n')
      .filter(Boolean)
      .map((line) => {
        const [metadata, path] = line.split('\t');
        return metadata?.startsWith('160000 ') ? path : null;
      })
      .filter(Boolean),
  );
}

function deploymentPathForGenerated(generatedPath) {
  const marker = '/generated/';
  const markerIndex = generatedPath.indexOf(marker);
  return markerIndex === -1 ? null : generatedPath.slice(0, markerIndex);
}

function unavailableGitlink(repoRoot, generatedPath, gitlinks) {
  const deploymentPath = deploymentPathForGenerated(generatedPath);
  if (!deploymentPath || !gitlinks.has(deploymentPath)) return false;
  return !existsSync(resolve(repoRoot, deploymentPath, '.git'));
}

function expectedArtifacts(publicPath) {
  return [
    publicPath,
    publicPath.replace(/\.json$/, '.html'),
    publicPath.replace(/\.json$/, '.internal.json'),
    publicPath.replace(/\.json$/, '.metadata.json'),
  ];
}

const repoRoot = findRepoRoot();
const generationOutput = runNode(repoRoot, 'remote/tools/generate-api-docs.mjs', ['--check']);
const validationOutput = runNode(repoRoot, 'remote/tools/validate-openapi-contracts.mjs');

const restApiPublicPath = 'remote/deployments/rest-api-rs/generated/api-docs.json';
const restApiInternalPath = 'remote/deployments/rest-api-rs/generated/api-docs.internal.json';
const restApiMetadataPath = 'remote/deployments/rest-api-rs/generated/api-docs.metadata.json';
const restApiPublic = readJson(repoRoot, restApiPublicPath);
const restApiInternal = readJson(repoRoot, restApiInternalPath);
const restApiMetadata = readJson(repoRoot, restApiMetadataPath);

assert.equal(restApiPublic.openapi, '3.1.0');
assert.equal(restApiPublic['x-dd-contract-scope'], 'public');
assert.equal(restApiInternal['x-dd-contract-scope'], 'internal');
const userGeneratedRoutes = restApiMetadata.routes.filter(
  (route) => route.routeType === 'user-generated',
);
assert.equal(
  restApiMetadata.routeTypeCounts['user-generated'],
  31,
  'method-aware metadata must count each user-generated HTTP operation',
);
assert.equal(
  new Set(userGeneratedRoutes.map((route) => route.path)).size,
  26,
  'the 31 user-generated operations must continue to cover 26 distinct paths',
);
for (const [path, methods] of [
  ['/api/agents/git-repos', ['GET', 'POST']],
  ['/api/graphql', ['GET', 'POST']],
  ['/api/lambdas/functions', ['GET', 'POST']],
  ['/api/lambdas/functions/:id', ['GET', 'PATCH']],
  ['/graphql', ['GET', 'POST']],
]) {
  const actualMethods = userGeneratedRoutes
    .filter((route) => route.path === path)
    .flatMap((route) => route.methods)
    .sort();
  assert.deepEqual(
    actualMethods,
    methods,
    `rest-api-rs generated metadata must preserve ${methods.join('/')} variants for ${path}`,
  );
}
assert.ok(!Object.prototype.hasOwnProperty.call(restApiMetadata.routeTypeCounts, 'pg' + '-first'));
for (const route of ['/docs/api', '/api/docs', '/api/docs.json']) {
  assert.ok(
    restApiMetadata.routes.some((entry) => entry.path === route && entry.methods.includes('GET')),
    `rest-api-rs generated metadata is missing GET ${route}`,
  );
  assert.ok(restApiInternal.paths[route]?.get, `rest-api-rs internal OpenAPI is missing GET ${route}`);
  assert.ok(restApiPublic.paths[route]?.get, `rest-api-rs public OpenAPI is missing GET ${route}`);
}
assert.ok(
  restApiMetadata.routes.every((route) => !route.path.startsWith('/api/db')),
  'rest-api-rs generated metadata must not expose generic /api/db routes.',
);
assert.ok(
  Object.keys(restApiPublic.paths).every((path) => !path.startsWith('/internal/')),
  'rest-api-rs public runtime OpenAPI must not expose /internal routes.',
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
const centralHtml = readFileSync(
  resolve(repoRoot, 'remote/deployments/generated-api-docs-index.html'),
  'utf8',
);
assert.ok(centralHtml.includes('Public-only fleet index'));
assert.ok(!centralHtml.includes('/internal/runtime-config'));

const gitlinks = deploymentGitlinks(repoRoot);
let checkedServices = 0;
let skippedGitlinks = 0;
for (const service of index.services) {
  const publicPath = service.generated?.[0];
  assert.ok(publicPath, `${service.service} must include a public runtime OpenAPI artifact`);
  assert.deepEqual(
    service.generated,
    expectedArtifacts(publicPath),
    `${service.service} central artifact list is not canonical`,
  );
  if (unavailableGitlink(repoRoot, publicPath, gitlinks)) {
    skippedGitlinks += 1;
    continue;
  }

  const internalPath = publicPath.replace(/\.json$/, '.internal.json');
  const metadataPath = publicPath.replace(/\.json$/, '.metadata.json');
  for (const artifactPath of [publicPath, internalPath, metadataPath]) {
    assert.ok(existsSync(resolve(repoRoot, artifactPath)), `${service.service} is missing ${artifactPath}`);
  }

  const publicOpenapi = readJson(repoRoot, publicPath);
  const internalOpenapi = readJson(repoRoot, internalPath);
  const metadata = readJson(repoRoot, metadataPath);
  assert.equal(publicOpenapi.openapi, '3.1.0', `${service.service} must emit OpenAPI 3.1`);
  assert.equal(publicOpenapi['x-dd-contract-scope'], 'public');
  assert.equal(internalOpenapi['x-dd-contract-scope'], 'internal');
  assert.equal(publicOpenapi['x-dd-service'], service.service);
  assert.equal(metadata.service, service.service);
  for (const route of metadata.standardDocsRoutes) {
    assert.ok(internalOpenapi.paths[route]?.get, `${service.service} internal OpenAPI is missing GET ${route}`);
    assert.ok(publicOpenapi.paths[route]?.get, `${service.service} public OpenAPI is missing GET ${route}`);
  }
  for (const path of Object.keys(publicOpenapi.paths)) {
    assert.ok(!path.startsWith('/internal/'), `${service.service} leaked internal route ${path}`);
  }
  checkedServices += 1;
}
assert.ok(checkedServices > 0, 'expected at least one available service contract to be checked');

console.log(
  [
    generationOutput,
    validationOutput,
    `route coverage checked ${checkedServices} service(s); skipped ${skippedGitlinks} uninitialized gitlink service(s)`,
  ]
    .filter(Boolean)
    .join('\n'),
);
