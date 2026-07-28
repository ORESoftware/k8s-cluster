#!/usr/bin/env node

import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  canonicalJson,
  flatOperations,
  loadSdkInputs,
  readRepoFile,
  repoRoot,
  sdkGeneratorPath,
  sha256,
} from './api-sdk-common.mjs';

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function readJson(path) {
  const raw = await readRepoFile(path);
  return { raw, value: JSON.parse(raw) };
}

function operationKey(operation) {
  return `${operation.service}\u0000${operation.operationId}\u0000${operation.method}\u0000${operation.path}`;
}

function compareOperations(expected, actual, label) {
  const expectedByKey = new Map(expected.map((operation) => [operationKey(operation), operation]));
  const actualByKey = new Map(actual.map((operation) => [operationKey(operation), operation]));
  assert(expectedByKey.size === expected.length, `${label}: expected operations contain duplicate keys`);
  assert(actualByKey.size === actual.length, `${label}: generated operations contain duplicate keys`);
  assert(
    JSON.stringify([...expectedByKey.keys()].sort()) === JSON.stringify([...actualByKey.keys()].sort()),
    `${label}: operation key set drifted from OpenAPI`,
  );
  for (const [key, expectedOperation] of expectedByKey) {
    const actualOperation = actualByKey.get(key);
    assert(
      canonicalJson(actualOperation) === canonicalJson(expectedOperation),
      `${label}: operation metadata drifted for ${expectedOperation.operationId}`,
    );
  }
}

function scopeLockExpected(catalog, catalogPath, catalogRaw) {
  return {
    catalogPath,
    catalogSha256: catalog.catalogSha256,
    catalogFileSha256: sha256(catalogRaw),
    serviceCount: catalog.serviceCount,
    operationCount: catalog.operationCount,
    skippedServices: catalog.skippedServices,
    specs: catalog.services.map((service) => ({
      service: service.service,
      specPath: service.specPath,
      specSha256: service.specSha256,
      operationCount: service.operationCount,
    })),
  };
}

async function validatePackage(packageEntry, catalog, catalogRaw) {
  const manifestPath = packageEntry.manifestPath;
  assert(existsSync(resolve(repoRoot, manifestPath)), `missing package manifest ${manifestPath}`);
  const { raw: manifestRaw, value: manifest } = await readJson(manifestPath);
  assert(sha256(manifestRaw) === packageEntry.manifestSha256, `${manifestPath}: lock digest drifted`);
  assert(manifest.schemaVersion === 1, `${manifestPath}: unsupported schemaVersion`);
  assert(manifest.language === packageEntry.language, `${manifestPath}: language drifted`);
  assert(manifest.scope === packageEntry.scope, `${manifestPath}: scope drifted`);
  assert(manifest.packageName === packageEntry.packageName, `${manifestPath}: package name drifted`);
  assert(manifest.catalogSha256 === catalog.catalogSha256, `${manifestPath}: catalog digest drifted`);
  assert(manifest.catalogFileSha256 === sha256(catalogRaw), `${manifestPath}: catalog file digest drifted`);
  assert(manifest.serviceCount === catalog.serviceCount, `${manifestPath}: service count drifted`);
  assert(manifest.operationCount === catalog.operationCount, `${manifestPath}: operation count drifted`);
  assert(
    canonicalJson(manifest.skippedServices) === canonicalJson(catalog.skippedServices),
    `${manifestPath}: skipped service list drifted`,
  );

  const packageRoot = packageEntry.path;
  const generatedPaths = new Set();
  for (const file of manifest.generatedFiles ?? []) {
    assert(typeof file.path === 'string' && file.path.length > 0, `${manifestPath}: invalid file path`);
    assert(!generatedPaths.has(file.path), `${manifestPath}: duplicate generated file ${file.path}`);
    generatedPaths.add(file.path);
    const path = `${packageRoot}/${file.path}`;
    assert(existsSync(resolve(repoRoot, path)), `${manifestPath}: missing generated file ${path}`);
    const raw = await readRepoFile(path);
    assert(sha256(raw) === file.sha256, `${manifestPath}: generated file digest drifted for ${file.path}`);
  }
  assert(generatedPaths.size >= 2, `${manifestPath}: package has too few generated files`);

  const sourceByLanguage = {
    typescript: 'src/index.ts',
    rust: 'src/lib.rs',
    dart: 'lib/dd_api_sdk.dart',
    gleam: 'src/dd_api_sdk.gleam',
  };
  const sourcePath = `${packageRoot}/${sourceByLanguage[manifest.language]}`;
  const source = await readRepoFile(sourcePath);
  assert(source.includes(catalog.catalogSha256), `${sourcePath}: source omits catalog digest`);
  assert(source.includes(String(catalog.operationCount)), `${sourcePath}: source omits operation count`);
}

async function main() {
  const inputs = await loadSdkInputs();
  const generatorRaw = await readRepoFile(sdkGeneratorPath);
  const catalogs = {};
  const catalogRaws = {};
  for (const scope of ['public', 'internal']) {
    const path = `remote/api-sdks/contracts/${scope}.json`;
    const { raw, value } = await readJson(path);
    catalogs[scope] = value;
    catalogRaws[scope] = raw;
    assert(
      canonicalJson(value) === canonicalJson(inputs.catalogs[scope]),
      `${scope} SDK catalog drifted from OpenAPI inputs`,
    );
    compareOperations(
      flatOperations(inputs.catalogs[scope]),
      flatOperations(value),
      `${scope} SDK catalog`,
    );
  }

  const publicOperations = flatOperations(catalogs.public);
  const internalByKey = new Map(
    flatOperations(catalogs.internal).map((operation) => [operationKey(operation), operation]),
  );
  for (const operation of publicOperations) {
    assert(operation.visibility === 'public', `public SDK leaked ${operation.operationId}`);
    const internal = internalByKey.get(operationKey(operation));
    assert(internal, `public SDK operation is absent from internal SDK: ${operation.operationId}`);
  }

  const { value: lock } = await readJson('remote/api-sdks/sdk-lock.json');
  assert(lock.schemaVersion === 1, 'sdk-lock.json has unsupported schemaVersion');
  assert(lock.generatedBy === sdkGeneratorPath, 'sdk-lock.json generator path drifted');
  assert(lock.generatorSha256 === sha256(generatorRaw), 'sdk-lock.json generator digest drifted');
  assert(lock.indexSha256 === inputs.indexSha256, 'sdk-lock.json API index digest drifted');
  for (const scope of ['public', 'internal']) {
    const expected = scopeLockExpected(
      catalogs[scope],
      `remote/api-sdks/contracts/${scope}.json`,
      catalogRaws[scope],
    );
    assert(
      canonicalJson(lock.scopes?.[scope]) === canonicalJson(expected),
      `sdk-lock.json ${scope} scope drifted`,
    );
  }

  assert(Array.isArray(lock.packages), 'sdk-lock.json packages must be an array');
  assert(lock.packages.length === 8, `sdk-lock.json must contain 8 packages, found ${lock.packages.length}`);
  const packageKeys = new Set();
  for (const packageEntry of lock.packages) {
    const key = `${packageEntry.language}:${packageEntry.scope}`;
    assert(!packageKeys.has(key), `sdk-lock.json duplicate package ${key}`);
    packageKeys.add(key);
    assert(['typescript', 'rust', 'dart', 'gleam'].includes(packageEntry.language), `${key}: unknown language`);
    assert(['public', 'internal'].includes(packageEntry.scope), `${key}: unknown scope`);
    assert(
      packageEntry.catalogSha256 === catalogs[packageEntry.scope].catalogSha256,
      `${key}: lock catalog digest drifted`,
    );
    assert(
      packageEntry.operationCount === catalogs[packageEntry.scope].operationCount,
      `${key}: lock operation count drifted`,
    );
    await validatePackage(packageEntry, catalogs[packageEntry.scope], catalogRaws[packageEntry.scope]);
  }

  for (const language of ['typescript', 'rust', 'dart', 'gleam']) {
    for (const scope of ['public', 'internal']) {
      assert(packageKeys.has(`${language}:${scope}`), `missing ${language}:${scope} package`);
    }
  }

  const publicSkipped = canonicalJson(catalogs.public.skippedServices);
  const internalSkipped = canonicalJson(catalogs.internal.skippedServices);
  assert(publicSkipped === internalSkipped, 'public/internal skipped service sets differ');
  assert(
    publicSkipped === canonicalJson(inputs.skippedServices),
    'generated skipped service set differs from unavailable deployment gitlinks',
  );

  console.log(
    `validated 8 packages from ${catalogs.internal.serviceCount} available service(s): ` +
      `${catalogs.public.operationCount} public and ${catalogs.internal.operationCount} internal operations; ` +
      `${inputs.skippedServices.length} uninitialized gitlink service(s) recorded`,
  );
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
