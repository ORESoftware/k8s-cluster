#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..', '..');
const indexPath = resolve(repoRoot, 'remote/deployments/generated-api-docs-index.json');
const manifestPath = resolve(repoRoot, 'remote/config/api-contracts.json');
const nativeManifestPath = resolve(repoRoot, 'remote/api-contracts/manifest.json');
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

function expectedGeneratedArtifacts(publicRelative) {
  return [
    publicRelative,
    publicRelative.replace(/\.json$/, '.html'),
    publicRelative.replace(/\.json$/, '.internal.json'),
    publicRelative.replace(/\.json$/, '.metadata.json'),
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
  const publicRelative = item.generated?.[0];
  assert(
    typeof publicRelative === 'string' &&
      publicRelative.endsWith('.json') &&
      !publicRelative.endsWith('.internal.json') &&
      !publicRelative.endsWith('.metadata.json'),
    `${item.service}: central index must identify the public runtime OpenAPI artifact`,
  );
  assert(
    JSON.stringify(item.generated) === JSON.stringify(expectedGeneratedArtifacts(publicRelative)),
    `${item.service}: central index generated artifacts must list public JSON, public HTML, internal JSON, and metadata JSON in canonical order`,
  );
  if (unavailableGitlink(publicRelative, gitlinks)) {
    return { skipped: true };
  }

  const publicPath = resolve(repoRoot, publicRelative);
  const internalPath = resolve(repoRoot, publicRelative.replace(/\.json$/, '.internal.json'));
  const metadataPath = resolve(repoRoot, publicRelative.replace(/\.json$/, '.metadata.json'));
  for (const artifactPath of [publicPath, internalPath, metadataPath]) {
    assert(existsSync(artifactPath), `${item.service}: missing ${displayPath(artifactPath)}`);
  }

  const publicOpenapi = readJson(publicPath);
  const internalOpenapi = readJson(internalPath);
  const metadata = readJson(metadataPath);
  verifyOpenApiShape(publicOpenapi, item.service, publicPath);
  verifyOpenApiShape(internalOpenapi, item.service, internalPath);
  assert(
    publicOpenapi['x-dd-contract-scope'] === 'public',
    `${item.service}: runtime OpenAPI must be the public contract`,
  );
  assert(
    internalOpenapi['x-dd-contract-scope'] === 'internal',
    `${item.service}: private SDK artifact must be the internal contract`,
  );
  assert(metadata.service === item.service, `${item.service}: metadata service mismatch`);
  assert(metadata.language === item.language, `${item.service}: metadata language mismatch`);
  assert(
    metadata.routeCount === metadata.routes?.length,
    `${item.service}: metadata routeCount is stale`,
  );

  const fullEntries = operationEntries(internalOpenapi);
  const actualKeys = fullEntries.flatMap(operationSourceKeys).sort();
  const expectedKeys = expectedRouteKeys(metadata);
  assert(
    JSON.stringify(actualKeys) === JSON.stringify(expectedKeys),
    `${item.service}: internal OpenAPI route/method set drifted from generated route metadata`,
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
      `${item.service}: internal OpenAPI omits GET ${route}`,
    );
  }

  const publicEntries = operationEntries(publicOpenapi);
  assert(
    Object.keys(publicOpenapi.components?.securitySchemes ?? {}).length === 0,
    `${item.service}: public runtime OpenAPI must not publish internal security schemes`,
  );
  const publicTagNames = new Set(publicEntries.flatMap((entry) => entry.operation.tags ?? []));
  assert(
    (publicOpenapi.tags ?? []).every((tag) => publicTagNames.has(tag.name)),
    `${item.service}: public runtime OpenAPI contains unused or internal tags`,
  );
  const fullByKey = new Map(fullEntries.map((entry) => [operationDocumentKey(entry), entry]));
  for (const entry of publicEntries) {
    const key = operationDocumentKey(entry);
    assert(fullByKey.has(key), `${item.service}: public OpenAPI contains non-canonical operation ${key}`);
    assert(
      entry.operation['x-dd-visibility'] === 'public',
      `${item.service}: internal operation leaked into runtime OpenAPI: ${key}`,
    );
    for (const extension of [
      'x-dd-auth',
      'x-dd-handlers',
      'x-dd-implementation',
      'x-dd-source-files',
      'x-dd-source-path',
      'x-dd-source-paths',
    ]) {
      assert(
        !Object.hasOwn(entry.operation, extension),
        `${item.service}: public runtime OpenAPI leaked debug extension ${extension} for ${key}`,
      );
    }
  }
  const expectedPublicKeys = fullEntries
    .filter((entry) => entry.operation['x-dd-visibility'] === 'public')
    .map(operationDocumentKey)
    .sort();
  const actualPublicKeys = publicEntries.map(operationDocumentKey).sort();
  assert(
    JSON.stringify(expectedPublicKeys) === JSON.stringify(actualPublicKeys),
    `${item.service}: runtime OpenAPI is not the exact public subset`,
  );
  for (const route of ['/docs/api', '/api/docs', '/api/docs.json']) {
    assert(
      actualPublicKeys.includes(`GET ${route}`),
      `${item.service}: public runtime OpenAPI omits GET ${route}`,
    );
  }

  return {
    skipped: false,
    operations: fullEntries.length,
    publicOperations: publicEntries.length,
  };
}

function main() {
  const index = readJson(indexPath);
  const manifest = readJson(manifestPath);
  const nativeManifest = readJson(nativeManifestPath);
  const indexedServices = (index.services ?? []).map((item) => item.service).sort();
  const allowlistedServices = [...(manifest.legacySourceScannerAllowlist ?? [])].sort();
  const nativeServices = Object.keys(nativeManifest.services ?? {}).sort();
  const nativeServiceSet = new Set(nativeServices);
  const overlap = allowlistedServices.filter((service) => nativeServiceSet.has(service));
  assert(
    overlap.length === 0,
    `services cannot use both native contracts and the legacy scanner: ${overlap.join(', ')}`,
  );
  const expectedServices = [...new Set([...allowlistedServices, ...nativeServices])].sort();
  const indexedServiceSet = new Set(indexedServices);
  const expectedServiceSet = new Set(expectedServices);
  const missingFromIndex = expectedServices.filter((service) => !indexedServiceSet.has(service));
  const unclassifiedInIndex = indexedServices.filter((service) => !expectedServiceSet.has(service));
  assert(
    missingFromIndex.length === 0 && unclassifiedInIndex.length === 0,
    `API contract service classification drift; missing from index: [${missingFromIndex.join(', ')}]; unclassified in index: [${unclassifiedInIndex.join(', ')}]`,
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
