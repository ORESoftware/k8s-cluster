import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
export const repoRoot = resolve(scriptDir, '..', '..');
export const sdkOutputRoot = resolve(repoRoot, 'remote/api-sdks');
export const sdkIndexPath = 'remote/deployments/generated-api-docs-index.json';
export const sdkGeneratorPath = 'remote/tools/generate-api-sdks.mjs';
export const HTTP_METHODS = ['get', 'put', 'post', 'delete', 'options', 'head', 'patch', 'trace'];

export function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}

export function stableValue(value) {
  if (Array.isArray(value)) {
    return value.map(stableValue);
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableValue(value[key])]),
    );
  }
  return value;
}

export function canonicalJson(value) {
  return JSON.stringify(stableValue(value));
}

export function prettyJson(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

export async function readRepoFile(path) {
  return readFile(resolve(repoRoot, path), 'utf8');
}

export async function readRepoJson(path) {
  const raw = await readRepoFile(path);
  return { raw, value: JSON.parse(raw) };
}

function parseGitlinks() {
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

function parseGitmoduleRepositories(raw) {
  const repositories = new Map();
  let path = null;
  let url = null;
  const flush = () => {
    if (path && url) {
      repositories.set(path, url);
    }
    path = null;
    url = null;
  };
  for (const line of raw.split('\n')) {
    if (line.trimStart().startsWith('[submodule ')) {
      flush();
    } else if (line.trimStart().startsWith('path = ')) {
      path = line.split('=', 2)[1].trim();
    } else if (line.trimStart().startsWith('url = ')) {
      url = line.split('=', 2)[1].trim();
    }
  }
  flush();
  return repositories;
}

function deploymentPathForGenerated(path) {
  const marker = '/generated/';
  const index = path.indexOf(marker);
  return index === -1 ? null : path.slice(0, index);
}

function unavailableGitlink(path, gitlinks) {
  const deploymentPath = deploymentPathForGenerated(path);
  if (!deploymentPath || !gitlinks.has(deploymentPath)) {
    return false;
  }
  return !existsSync(resolve(repoRoot, deploymentPath, '.git'));
}

function resolveLocalRef(document, value) {
  if (!value || typeof value !== 'object' || typeof value.$ref !== 'string') {
    return value;
  }
  if (!value.$ref.startsWith('#/')) {
    throw new Error(`remote OpenAPI references are not supported by the fleet SDK generator: ${value.$ref}`);
  }
  let current = document;
  for (const token of value.$ref.slice(2).split('/')) {
    const key = token.replaceAll('~1', '/').replaceAll('~0', '~');
    current = current?.[key];
    if (current === undefined) {
      throw new Error(`unresolvable OpenAPI reference: ${value.$ref}`);
    }
  }
  return current;
}

function mergedParameters(document, pathItem, operation) {
  const parameters = new Map();
  for (const candidate of [...(pathItem.parameters ?? []), ...(operation.parameters ?? [])]) {
    const parameter = resolveLocalRef(document, candidate);
    if (!parameter || typeof parameter.name !== 'string' || typeof parameter.in !== 'string') {
      throw new Error('OpenAPI parameter is missing name or in');
    }
    parameters.set(`${parameter.in}:${parameter.name}`, parameter);
  }
  return [...parameters.values()]
    .filter((parameter) => parameter.in === 'path' || parameter.in === 'query')
    .sort((left, right) => `${left.in}:${left.name}`.localeCompare(`${right.in}:${right.name}`));
}

function contentMetadata(document, content) {
  return Object.entries(content ?? {})
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([contentType, mediaType]) => ({
      contentType,
      schemaSha256: sha256(canonicalJson(resolveLocalRef(document, mediaType?.schema ?? {}))),
    }));
}

function extractOperations(document, service, scope) {
  const operations = [];
  const operationIds = new Set();
  for (const path of Object.keys(document.paths ?? {}).sort()) {
    const pathItem = document.paths[path] ?? {};
    for (const method of HTTP_METHODS) {
      const operation = pathItem[method];
      if (!operation) {
        continue;
      }
      const operationId = operation.operationId;
      if (typeof operationId !== 'string' || operationId.length === 0) {
        throw new Error(`${service.service}: ${method.toUpperCase()} ${path} has no operationId`);
      }
      if (operationIds.has(operationId)) {
        throw new Error(`${service.service}: duplicate operationId ${operationId}`);
      }
      operationIds.add(operationId);
      const parameters = mergedParameters(document, pathItem, operation);
      const pathParameters = parameters
        .filter((parameter) => parameter.in === 'path')
        .map((parameter) => ({
          name: parameter.name,
          required: true,
          schemaSha256: sha256(canonicalJson(resolveLocalRef(document, parameter.schema ?? {}))),
        }));
      const queryParameters = parameters
        .filter((parameter) => parameter.in === 'query')
        .map((parameter) => ({
          name: parameter.name,
          required: parameter.required === true,
          schemaSha256: sha256(canonicalJson(resolveLocalRef(document, parameter.schema ?? {}))),
        }));
      const requestBody = resolveLocalRef(document, operation.requestBody);
      const responses = Object.entries(operation.responses ?? {})
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([status, responseValue]) => {
          const response = resolveLocalRef(document, responseValue);
          return {
            status,
            content: contentMetadata(document, response?.content),
          };
        });
      const visibility = operation['x-dd-visibility'] ?? scope;
      if (scope === 'public' && visibility !== 'public') {
        throw new Error(`${service.service}: public OpenAPI contains non-public operation ${operationId}`);
      }
      const securityNames = (operation.security ?? [])
        .flatMap((entry) => Object.keys(entry))
        .sort();
      operations.push({
        service: service.service,
        serviceLanguage: service.language,
        operationId,
        method: method.toUpperCase(),
        path,
        summary: operation.summary ?? '',
        visibility,
        routeType: operation['x-dd-route-type'] ?? 'unspecified',
        auth:
          operation['x-dd-auth'] ??
          (scope === 'public' ? 'public' : securityNames.join(',') || 'service-defined'),
        pathParameters,
        queryParameters,
        requestBody: {
          required: requestBody?.required === true,
          content: contentMetadata(document, requestBody?.content),
        },
        responses,
        contractSha256: sha256(
          canonicalJson({
            path,
            method,
            pathParameters: pathItem.parameters ?? [],
            operation,
          }),
        ),
      });
    }
  }
  return operations.sort((left, right) => {
    return `${left.service}:${left.operationId}`.localeCompare(`${right.service}:${right.operationId}`);
  });
}

function buildCatalog(scope, services, skippedServices) {
  const core = {
    schemaVersion: 1,
    scope,
    generatedBy: sdkGeneratorPath,
    services,
    skippedServices,
  };
  return {
    ...core,
    serviceCount: services.length,
    operationCount: services.reduce((sum, service) => sum + service.operationCount, 0),
    catalogSha256: sha256(canonicalJson(core)),
  };
}

export async function loadSdkInputs() {
  const [{ raw: indexRaw, value: index }, gitmodulesRaw] = await Promise.all([
    readRepoJson(sdkIndexPath),
    readRepoFile('.gitmodules'),
  ]);
  if (!Array.isArray(index.services) || index.services.length === 0) {
    throw new Error('generated API docs index has no services');
  }
  const gitlinks = parseGitlinks();
  const repositories = parseGitmoduleRepositories(gitmodulesRaw);
  const available = [];
  const skippedServices = [];
  const fleetOperationIds = new Set();

  for (const service of [...index.services].sort((left, right) => left.service.localeCompare(right.service))) {
    const publicPath = service.generated?.[0];
    const internalPath = service.generated?.[2];
    if (typeof publicPath !== 'string' || typeof internalPath !== 'string') {
      throw new Error(`${service.service}: central index is missing public or internal OpenAPI path`);
    }
    const publicExists = existsSync(resolve(repoRoot, publicPath));
    const internalExists = existsSync(resolve(repoRoot, internalPath));
    if (!publicExists || !internalExists) {
      if (unavailableGitlink(publicPath, gitlinks) && unavailableGitlink(internalPath, gitlinks)) {
        const deploymentPath = deploymentPathForGenerated(publicPath);
        skippedServices.push({
          service: service.service,
          language: service.language,
          deploymentPath,
          sourceRepository: repositories.get(deploymentPath) ?? 'unknown',
          reason: 'uninitialized-deployment-gitlink',
        });
        continue;
      }
      throw new Error(
        `${service.service}: OpenAPI artifacts are missing outside an unavailable deployment gitlink`,
      );
    }

    const [{ raw: publicRaw, value: publicDocument }, { raw: internalRaw, value: internalDocument }] =
      await Promise.all([readRepoJson(publicPath), readRepoJson(internalPath)]);
    for (const [scope, document, path] of [
      ['public', publicDocument, publicPath],
      ['internal', internalDocument, internalPath],
    ]) {
      if (document.openapi !== '3.1.0') {
        throw new Error(`${service.service}: ${path} is not OpenAPI 3.1.0`);
      }
      if (document['x-dd-service'] !== service.service) {
        throw new Error(`${service.service}: ${path} service identity drifted`);
      }
      if (document['x-dd-contract-scope'] !== scope) {
        throw new Error(`${service.service}: ${path} scope drifted from ${scope}`);
      }
    }

    const publicOperations = extractOperations(publicDocument, service, 'public');
    const internalOperations = extractOperations(internalDocument, service, 'internal');
    for (const operation of internalOperations) {
      if (fleetOperationIds.has(operation.operationId)) {
        throw new Error(`fleet duplicate operationId ${operation.operationId}`);
      }
      fleetOperationIds.add(operation.operationId);
    }
    const internalKeys = new Set(
      internalOperations.map((operation) => `${operation.method} ${operation.path}`),
    );
    for (const operation of publicOperations) {
      if (!internalKeys.has(`${operation.method} ${operation.path}`)) {
        throw new Error(`${service.service}: public operation is absent from internal contract`);
      }
    }

    available.push({
      service,
      public: {
        specPath: publicPath,
        specSha256: sha256(publicRaw),
        operations: publicOperations,
      },
      internal: {
        specPath: internalPath,
        specSha256: sha256(internalRaw),
        operations: internalOperations,
      },
    });
  }

  skippedServices.sort((left, right) => left.service.localeCompare(right.service));
  const catalogs = {};
  for (const scope of ['public', 'internal']) {
    const services = available.map((entry) => ({
      service: entry.service.service,
      language: entry.service.language,
      specPath: entry[scope].specPath,
      specSha256: entry[scope].specSha256,
      operationCount: entry[scope].operations.length,
      operations: entry[scope].operations,
    }));
    catalogs[scope] = buildCatalog(scope, services, skippedServices);
  }

  return {
    index,
    indexRaw,
    indexSha256: sha256(indexRaw),
    catalogs,
    skippedServices,
  };
}

export function flatOperations(catalog) {
  return catalog.services.flatMap((service) => service.operations);
}

export function runtimeOperations(catalog) {
  return flatOperations(catalog).map((operation) => ({
    service: operation.service,
    operationId: operation.operationId,
    method: operation.method,
    path: operation.path,
    pathParameters: operation.pathParameters.map((parameter) => parameter.name),
    requiredQueryParameters: operation.queryParameters
      .filter((parameter) => parameter.required)
      .map((parameter) => parameter.name),
    optionalQueryParameters: operation.queryParameters
      .filter((parameter) => !parameter.required)
      .map((parameter) => parameter.name),
    requestBodyRequired: operation.requestBody.required,
    contractSha256: operation.contractSha256,
  }));
}
