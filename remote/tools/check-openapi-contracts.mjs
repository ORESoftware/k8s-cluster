#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const manifest = JSON.parse(
  await readFile(resolve(repoRoot, 'remote/api-contracts/manifest.json'), 'utf8'),
);
const serviceIndex = process.argv.indexOf('--service');
const requested = serviceIndex >= 0 ? process.argv[serviceIndex + 1] : undefined;
if (serviceIndex >= 0 && (!requested || requested.startsWith('--'))) {
  throw new Error('--service requires a service name');
}

const HTTP_METHODS = new Set([
  'get',
  'post',
  'put',
  'patch',
  'delete',
  'head',
  'options',
  'trace',
]);
const BODY_METHODS = new Set(['post', 'put', 'patch']);
const PUBLIC_PATHS = new Set([
  '/',
  '/healthz',
  '/livez',
  '/readyz',
  '/metrics',
  '/openapi.json',
  '/api/docs.json',
  '/api/docs',
  '/docs/api',
]);

function runExport(service) {
  const [command, ...args] = service.export;
  return execFileSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
    env: {
      ...process.env,
      RUST_BACKTRACE: '1',
    },
    maxBuffer: 64 * 1024 * 1024,
  });
}

function decodePointerToken(token) {
  return token.replaceAll('~1', '/').replaceAll('~0', '~');
}

function resolveLocalRef(document, ref) {
  if (typeof ref !== 'string' || !ref.startsWith('#/')) {
    throw new Error(`external or malformed $ref is not allowed in generated SDK contracts: ${ref}`);
  }
  let current = document;
  for (const rawToken of ref.slice(2).split('/')) {
    const token = decodePointerToken(rawToken);
    if (current === null || typeof current !== 'object' || !(token in current)) {
      return false;
    }
    current = current[token];
  }
  return true;
}

function assertLocalRefsResolve(name, document) {
  const stack = [[document, '$']];
  while (stack.length > 0) {
    const [node, location] = stack.pop();
    if (node === null || typeof node !== 'object') continue;
    if (!Array.isArray(node) && Object.hasOwn(node, '$ref')) {
      const ref = node.$ref;
      if (!resolveLocalRef(document, ref)) {
        throw new Error(`${name}: unresolved local $ref ${ref} at ${location}`);
      }
    }
    for (const [key, value] of Object.entries(node)) {
      stack.push([value, `${location}.${key}`]);
    }
  }
}

function assertNoGeneratorInfoLeak(name, document) {
  const contact = document.info?.contact;
  const license = document.info?.license;
  const utoipaContactLeaked =
    contact?.name === 'Juha Kukkonen' || contact?.email === 'juha7kukkonen@gmail.com';
  const utoipaLicenseLeaked =
    license?.name === 'MIT OR Apache-2.0' && license?.identifier === 'MIT OR Apache-2.0';
  if (utoipaContactLeaked || utoipaLicenseLeaked) {
    throw new Error(
      `${name}: OpenAPI info contains Utoipa dependency metadata instead of explicit service provenance`,
    );
  }
}

function validate(name, service, raw) {
  const document = JSON.parse(raw);
  if (typeof document.openapi !== 'string' || !document.openapi.startsWith('3.1.')) {
    throw new Error(`${name}: expected OpenAPI 3.1.x, got ${document.openapi}`);
  }
  if (!document.info?.title || !document.info?.version) {
    throw new Error(`${name}: info.title and info.version are required`);
  }
  assertNoGeneratorInfoLeak(name, document);
  const securitySchemes = document.components?.securitySchemes ?? {};
  if (
    securitySchemes === null ||
    typeof securitySchemes !== 'object' ||
    Array.isArray(securitySchemes)
  ) {
    throw new Error(`${name}: components.securitySchemes must be an object when present`);
  }
  assertLocalRefsResolve(name, document);
  for (const route of service.docsRoutes) {
    if (!document.paths?.[route]?.get) {
      throw new Error(`${name}: standard docs GET route ${route} is missing`);
    }
  }

  const operationIds = new Set();
  let operations = 0;
  for (const [path, pathItem] of Object.entries(document.paths ?? {})) {
    for (const [method, operation] of Object.entries(pathItem ?? {})) {
      if (!HTTP_METHODS.has(method)) continue;
      operations += 1;
      if (!operation || typeof operation !== 'object') {
        throw new Error(`${name}: ${method.toUpperCase()} ${path} is not an operation`);
      }
      if (typeof operation.operationId !== 'string' || operation.operationId.length === 0) {
        throw new Error(`${name}: ${method.toUpperCase()} ${path} has no operationId`);
      }
      if (operationIds.has(operation.operationId)) {
        throw new Error(`${name}: duplicate operationId ${operation.operationId}`);
      }
      operationIds.add(operation.operationId);
      if (!operation.responses || Object.keys(operation.responses).length === 0) {
        throw new Error(`${name}: ${method.toUpperCase()} ${path} has no responses`);
      }
      if (operation.requestBody !== undefined) {
        if (
          !operation.requestBody ||
          typeof operation.requestBody !== 'object' ||
          Array.isArray(operation.requestBody) ||
          !operation.requestBody.content ||
          typeof operation.requestBody.content !== 'object' ||
          Object.keys(operation.requestBody.content).length === 0
        ) {
          throw new Error(`${name}: ${method.toUpperCase()} ${path} has a malformed requestBody`);
        }
      }
      const security = operation.security;
      if (security !== undefined) {
        if (!Array.isArray(security)) {
          throw new Error(`${name}: ${method.toUpperCase()} ${path} security must be an array`);
        }
        for (const requirement of security) {
          if (!requirement || typeof requirement !== 'object' || Array.isArray(requirement)) {
            throw new Error(`${name}: ${method.toUpperCase()} ${path} has an invalid security requirement`);
          }
          for (const scheme of Object.keys(requirement)) {
            if (!Object.hasOwn(securitySchemes, scheme)) {
              throw new Error(
                `${name}: ${method.toUpperCase()} ${path} references undeclared security scheme ${scheme}`,
              );
            }
          }
        }
      }
      if (!PUBLIC_PATHS.has(path)) {
        const networkBoundary =
          operation['x-dd-auth'] === 'cluster-network-policy' &&
          (security === undefined || (Array.isArray(security) && security.length === 0));
        if ((!Array.isArray(security) || security.length === 0) && !networkBoundary) {
          throw new Error(`${name}: ${method.toUpperCase()} ${path} has no security requirement`);
        }
      }
    }
  }
  if (operations === 0) {
    throw new Error(`${name}: no operations discovered`);
  }
  return { document, operations, schemas: Object.keys(document.components?.schemas ?? {}).length };
}

for (const [name, service] of Object.entries(manifest.services)) {
  if (requested && requested !== name) continue;
  const first = runExport(service);
  const second = runExport(service);
  if (first !== second) {
    throw new Error(`${name}: two consecutive exports were not byte-identical`);
  }
  const committed = await readFile(resolve(repoRoot, service.contract), 'utf8');
  if (first !== committed) {
    throw new Error(
      `${name}: ${service.contract} is stale; rerun the service exporter and commit the exact bytes`,
    );
  }
  const summary = validate(name, service, first);
  console.log(
    `${name}: OpenAPI contract verified (${summary.operations} operations, ${summary.schemas} schemas)`,
  );
}
