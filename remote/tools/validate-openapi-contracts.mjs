#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..', '..');
const indexPath = resolve(repoRoot, 'remote/deployments/generated-api-docs-index.json');
const manifestPath = resolve(repoRoot, 'remote/config/api-contracts.json');
const HTTP_METHODS = ['get', 'put', 'post', 'delete', 'options', 'head', 'patch', 'trace'];

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

function displayPath(path) {
  return relative(repoRoot, path).split(sep).join('/');
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function deploymentGitlinks() {
  const output = execFileSync(
    'git',
    ['ls-files', '--stage', '--', 'remote/deployments'],
    { cwd: repoRoot, encoding: 'utf8' },
  );
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

function unavailableGitlink(generatedPath, gitlinks) {
  const deploymentPath = deploymentPathForGenerated(generatedPath);
  if (!deploymentPath || !gitlinks.has(deploymentPath)) {
    return false;
  }
  return !existsSync(resolve(repoRoot, deploymentPath, '.git'));
}

function operationEntries(document) {
  const entries = [];
  for (const [path, pathItem] of Object.entries(document.paths ?? {})) {
    for (const method of HTTP_METHODS) {
      const operation = pathItem?.[method];
      if (operation) {
        entries.push({ method: method.toUpperCase(), path, operation });
      }
    }
  }
  return entries;
}

function operationSourceKeys(entry) {
  const sourcePaths = entry.operation['x-dd-source-paths'] ?? [entry.operation['x-dd-source-path']];
  assert(
    Array.isArray(sourcePaths) && sourcePaths.length > 0,
    `${entry.operation.operationId ?? `${entry.method} ${entry.path}`} is missing x-dd-source-paths`,
  );
  for (const sourcePath of sourcePaths) {
    assert(
      typeof sourcePath === 'string' && sourcePath.startsWith('/'),
      `${entry.operation.operationId ?? `${entry.method} ${entry.path}`} has an invalid source path`,
    );
  }
  return [...new Set(sourcePaths)].sort().map((sourcePath) => `${entry.method} ${sourcePath}`);
}

function operationDocumentKey(entry) {
  return `${entry.method} ${entry.path}`;
}

function expectedRouteKeys(metadata) {
  const keys = [];
  for (const route of metadata.routes ?? []) {
    for (const method of route.methods ?? []) {
      keys.push(`${method} ${route.path}`);
    }
  }
  return keys.sort();
}

function expectedGeneratedArtifacts(openapiRelative) {
  return [
    openapiRelative,
    openapiRelative.replace(/\.json$/, '.html'),
    openapiRelative.replace(/\.json$/, '.public.json'),
    openapiRelative.replace(/\.json$/, '.metadata.json'),
  ];
}

function verifyOpenApiShape(document, service, sourcePath) {
  assert(
    document.openapi === '3.1.0',
    `${service}: ${displayPath(sourcePath)} must declare OpenAPI 3.1.0`,
  );
  assert(
    typeof document.info?.title === 'string' && document.info.title.length > 0,
    `${service}: OpenAPI info.title is required`,
  );
  assert(
    document.paths && typeof document.paths === 'object' && !Array.isArray(document.paths),
    `${service}: OpenAPI paths object is required`,
  );
  assert(!Object.hasOwn(document, 'routes'), `${service}: legacy routes must live in metadata, not OpenAPI`);
  assert(
    document['x-dd-service'] === service,
    `${service}: x-dd-service must match the central index`,
  );
}

function verifyService(item, gitlinks, fleetOperationIds) {
  const openapiRelative = item.generated?.[0];
  assert(
    typeof openapiRelative === 'string' && openapiRelative.endsWith('.json'),
    `${item.service}: central index must identify the canonical JSON artifact`,
  );
  assert(
    JSON.stringify(item.generated) === JSON.stringify(expectedGeneratedArtifacts(openapiRelative)),
    `${item.service}: central index generated artifacts must list full JSON, HTML, public JSON, and metadata JSON in canonical order`,
  );
  if (unavailableGitlink(openapiRelative, gitlinks)) {
    return { skipped: true };
  }

  const openapiPath = resolve(repoRoot, openapiRelative);
  const publicPath = resolve(repoRoot, openapiRelative.replace(/\.json$/, '.public.json'));
  const metadataPath = resolve(repoRoot, openapiRelative.replace(/\.json$/, '.metadata.json'));
  for (const path of [openapiPath, publicPath, metadataPath]) {
    assert(existsSync(path), `${item.service}: missing ${displayPath(path)}`);
  }

  const openapi = readJson(openapiPath);
  const publicOpenapi = readJson(publicPath);
  const metadata = readJson(metadataPath);
  verifyOpenApiShape(openapi, item.service, openapiPath);
  verifyOpenApiShape(publicOpenapi, item.service, publicPath);
  assert(metadata.service === item.service, `${item.service}: metadata service mismatch`);
  assert(metadata.language === item.language, `${item.service}: metadata language mismatch`);
  assert(
    metadata.routeCount === metadata.routes?.length,
    `${item.service}: metadata routeCount is stale`,
  );

  const fullEntries = operationEntries(openapi);
  const actualKeys = fullEntries.flatMap(operationSourceKeys).sort();
  const expectedKeys = expectedRouteKeys(metadata);
  assert(
    JSON.stringify(actualKeys) === JSON.stringify(expectedKeys),
    `${item.service}: OpenAPI route/method set drifted from generated route metadata`,
  );

  const localOperationIds = new Set();
  for (const entry of fullEntries) {
    const operationId = entry.operation.operationId;
    assert(
      typeof operationId === 'string' && operationId.length > 0,
      `${item.service}: ${entry.method} ${entry.path} has no operationId`,
    );
    assert(!localOperationIds.has(operationId), `${item.service}: duplicate operationId ${operationId}`);
    assert(!fleetOperationIds.has(operationId), `fleet duplicate operationId ${operationId}`);
    localOperationIds.add(operationId);
    fleetOperationIds.add(operationId);
    assert(
      ['public', 'internal'].includes(entry.operation['x-dd-visibility']),
      `${item.service}: ${operationId} must declare x-dd-visibility`,
    );
    assert(
      typeof entry.operation['x-dd-auth'] === 'string',
      `${item.service}: ${operationId} must declare x-dd-auth`,
    );
  }

  const standardRoutes = new Set(metadata.standardDocsRoutes ?? []);
  for (const route of ['/docs/api', '/api/docs', '/api/docs.json']) {
    assert(standardRoutes.has(route), `${item.service}: metadata omits standard route ${route}`);
    assert(
      actualKeys.includes(`GET ${route}`),
      `${item.service}: OpenAPI omits GET ${route}`,
    );
  }

  const publicEntries = operationEntries(publicOpenapi);
  const fullByKey = new Map(fullEntries.map((entry) => [operationDocumentKey(entry), entry]));
  for (const entry of publicEntries) {
    const key = operationDocumentKey(entry);
    assert(fullByKey.has(key), `${item.service}: public OpenAPI contains non-canonical operation ${key}`);
    assert(
      JSON.stringify(operationSourceKeys(entry)) === JSON.stringify(operationSourceKeys(fullByKey.get(key))),
      `${item.service}: public OpenAPI source-path set drifted for ${key}`,
    );
    assert(
      entry.operation['x-dd-visibility'] === 'public',
      `${item.service}: internal operation leaked into public OpenAPI: ${key}`,
    );
  }
  const expectedPublicKeys = fullEntries
    .filter((entry) => entry.operation['x-dd-visibility'] === 'public')
    .map(operationDocumentKey)
    .sort();
  const actualPublicKeys = publicEntries.map(operationDocumentKey).sort();
  assert(
    JSON.stringify(expectedPublicKeys) === JSON.stringify(actualPublicKeys),
    `${item.service}: public OpenAPI is not the exact public subset`,
  );

  return {
    skipped: false,
    operations: fullEntries.length,
    publicOperations: publicEntries.length,
  };
}

function main() {
  const index = readJson(indexPath);
  const manifest = readJson(manifestPath);
  const indexedServices = (index.services ?? []).map((item) => item.service).sort();
  const allowlistedServices = [...(manifest.legacySourceScannerAllowlist ?? [])].sort();
  assert(
    JSON.stringify(indexedServices) === JSON.stringify(allowlistedServices),
    'API contract manifest and central service index differ; new services must choose a native source-of-truth strategy or be reviewed into the temporary scanner allowlist',
  );

  assert(
    manifest.standardRoutes?.openapi === '/api/docs.json',
    'canonical OpenAPI route must remain /api/docs.json',
  );

  const gitlinks = deploymentGitlinks();
  const fleetOperationIds = new Set();
  let checked = 0;
  let skipped = 0;
  let operations = 0;
  let publicOperations = 0;
  for (const item of index.services ?? []) {
    const result = verifyService(item, gitlinks, fleetOperationIds);
    if (result.skipped) {
      skipped += 1;
      continue;
    }
    checked += 1;
    operations += result.operations;
    publicOperations += result.publicOperations;
  }

  console.log(
    `validated OpenAPI contracts for ${checked} service(s): ${operations} operations, ${publicOperations} public; skipped ${skipped} uninitialized gitlink service(s)`,
  );
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
}
