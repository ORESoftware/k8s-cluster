#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const CANONICAL_DOC_ROUTES = [
  '/openapi.json',
  '/api/docs.json',
  '/api/docs',
  '/docs/api',
];
const JSON_DOC_ROUTES = new Set(['/openapi.json', '/api/docs.json']);
const HTML_DOC_ROUTES = new Set(['/api/docs', '/docs/api']);
const DEFAULT_TIMEOUT_MS = 10_000;
const DEFAULT_MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
const USER_AGENT = 'oresoftware-api-contract-conformance/1';

function usage() {
  return `Usage: node remote/tools/check-live-api-contract.mjs --service <name> --base-url <url> [options]

Options:
  --service <name>             Service key from remote/api-contracts/manifest.json.
  --base-url <url>             Root HTTP(S) URL for the running service.
  --timeout-ms <milliseconds>  Per-request timeout (default: ${DEFAULT_TIMEOUT_MS}).
  --max-response-bytes <n>     Maximum response body size (default: ${DEFAULT_MAX_RESPONSE_BYTES}).
  --json                       Emit the conformance report as JSON.
  --help                       Show this message.
`;
}

function positiveInteger(value, option) {
  if (!/^[1-9]\d*$/.test(value ?? '')) {
    throw new Error(`${option} requires a positive integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new Error(`${option} exceeds JavaScript's safe integer range`);
  }
  return parsed;
}

function parseArgs(argv) {
  const result = {
    service: null,
    baseUrl: null,
    timeoutMs: DEFAULT_TIMEOUT_MS,
    maxResponseBytes: DEFAULT_MAX_RESPONSE_BYTES,
    json: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--help') {
      process.stdout.write(usage());
      process.exit(0);
    }
    if (argument === '--json') {
      result.json = true;
      continue;
    }
    if (!['--service', '--base-url', '--timeout-ms', '--max-response-bytes'].includes(argument)) {
      throw new Error(`unknown argument: ${argument}\n${usage()}`);
    }
    const value = argv[index + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`${argument} requires a value`);
    }
    index += 1;
    if (argument === '--service') result.service = value;
    if (argument === '--base-url') result.baseUrl = value;
    if (argument === '--timeout-ms') result.timeoutMs = positiveInteger(value, argument);
    if (argument === '--max-response-bytes') {
      result.maxResponseBytes = positiveInteger(value, argument);
    }
  }

  if (!result.service || !result.baseUrl) {
    throw new Error(`--service and --base-url are required\n${usage()}`);
  }
  if (!/^[a-z0-9][a-z0-9._-]*$/.test(result.service)) {
    throw new Error(`invalid service key: ${result.service}`);
  }
  return result;
}

function findRepoRoot() {
  const scriptDirectory = dirname(fileURLToPath(import.meta.url));
  for (const candidate of [process.cwd(), resolve(scriptDirectory, '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/api-contracts/manifest.json'))) {
      return candidate;
    }
  }
  throw new Error(`unable to locate repository root from ${process.cwd()}`);
}

function assertRepoPath(repoRoot, path) {
  if (
    typeof path !== 'string' ||
    path.length === 0 ||
    path.includes('\0') ||
    path.startsWith('/') ||
    path.split('/').some((segment) => segment === '' || segment === '.' || segment === '..')
  ) {
    throw new Error(`unsafe repository path: ${String(path)}`);
  }
  const absolute = resolve(repoRoot, path);
  if (absolute !== repoRoot && !absolute.startsWith(`${repoRoot}${sep}`)) {
    throw new Error(`repository path escapes checkout: ${path}`);
  }
  return absolute;
}

function canonicalBaseUrl(raw) {
  let url;
  try {
    url = new URL(raw);
  } catch (error) {
    throw new Error(`invalid --base-url ${JSON.stringify(raw)}: ${error.message}`);
  }
  if (!['http:', 'https:'].includes(url.protocol)) {
    throw new Error(`--base-url must use http or https: ${url.protocol}`);
  }
  if (url.username || url.password) {
    throw new Error('--base-url must not contain credentials');
  }
  if (url.search || url.hash) {
    throw new Error('--base-url must not contain a query string or fragment');
  }
  if (url.pathname !== '/' && url.pathname !== '') {
    throw new Error('--base-url must identify the service root without a path prefix');
  }
  url.pathname = '/';
  return url;
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function assertCanonicalRoutes(service, routes) {
  if (!Array.isArray(routes)) {
    throw new Error(`${service} manifest entry has no docsRoutes array`);
  }
  const unique = [...new Set(routes)];
  if (unique.length !== routes.length) {
    throw new Error(`${service} docsRoutes contains duplicates`);
  }
  const actual = [...unique].sort();
  const expected = [...CANONICAL_DOC_ROUTES].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `${service} docsRoutes must be exactly ${JSON.stringify(CANONICAL_DOC_ROUTES)}; received ${JSON.stringify(routes)}`,
    );
  }
}

function assertPublicContract(service, document) {
  if (!document || typeof document !== 'object' || Array.isArray(document)) {
    throw new Error(`${service} public contract is not a JSON object`);
  }
  if (document.openapi !== '3.1.0') {
    throw new Error(`${service} public contract must use OpenAPI 3.1.0`);
  }
  if (document['x-dd-contract-scope'] !== 'public') {
    throw new Error(`${service} public contract is missing x-dd-contract-scope=public`);
  }
  if (document['x-dd-service'] !== service) {
    throw new Error(
      `${service} public contract identity drifted: ${JSON.stringify(document['x-dd-service'])}`,
    );
  }
  const paths = document.paths;
  if (!paths || typeof paths !== 'object' || Array.isArray(paths)) {
    throw new Error(`${service} public contract has no paths object`);
  }
  for (const route of CANONICAL_DOC_ROUTES) {
    if (!paths[route]?.get) {
      throw new Error(`${service} public contract is missing GET ${route}`);
    }
  }
  for (const path of Object.keys(paths)) {
    if (path.startsWith('/internal/')) {
      throw new Error(`${service} public contract contains internal path ${path}`);
    }
  }
}

function jsonContentType(value) {
  const mediaType = value?.split(';', 1)[0].trim().toLowerCase();
  return (
    mediaType === 'application/json' ||
    Boolean(mediaType?.startsWith('application/') && mediaType.endsWith('+json'))
  );
}

function htmlContentType(value) {
  return value?.split(';', 1)[0].trim().toLowerCase() === 'text/html';
}

async function readCappedBody(response, maximum) {
  const declared = response.headers.get('content-length');
  if (declared && /^\d+$/.test(declared) && Number(declared) > maximum) {
    throw new Error(`declared response body is ${declared} bytes; maximum is ${maximum}`);
  }
  if (!response.body) return Buffer.alloc(0);

  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maximum) {
      await reader.cancel('response body exceeded configured maximum');
      throw new Error(`response body exceeded ${maximum} bytes`);
    }
    chunks.push(Buffer.from(value));
  }
  return Buffer.concat(chunks, total);
}

async function fetchRoute(baseUrl, route, { timeoutMs, maxResponseBytes }) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const url = new URL(route, baseUrl);
  try {
    const response = await fetch(url, {
      method: 'GET',
      redirect: 'error',
      headers: {
        accept: JSON_DOC_ROUTES.has(route)
          ? 'application/openapi+json, application/json'
          : 'text/html',
        'user-agent': USER_AGENT,
      },
      signal: controller.signal,
    });
    if (response.status !== 200) {
      throw new Error(`GET ${route} returned HTTP ${response.status}`);
    }
    const body = await readCappedBody(response, maxResponseBytes);
    return {
      body,
      contentType: response.headers.get('content-type') ?? '',
      status: response.status,
      url: url.toString(),
    };
  } catch (error) {
    if (error.name === 'AbortError') {
      throw new Error(`GET ${route} exceeded ${timeoutMs} ms`);
    }
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

function assertHtmlBootstrap(service, route, body, internalDocsRoutes) {
  const html = body.toString('utf8');
  if (!html.includes('/openapi.json')) {
    throw new Error(`${service} GET ${route} does not bootstrap /openapi.json`);
  }
  const forbidden = new Set([
    '/internal/',
    'api-docs.internal.json',
    ...(internalDocsRoutes ?? []),
  ]);
  for (const value of forbidden) {
    if (value && html.includes(value)) {
      throw new Error(`${service} GET ${route} exposes internal documentation reference ${value}`);
    }
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const repoRoot = findRepoRoot();
  const manifestPath = assertRepoPath(repoRoot, 'remote/api-contracts/manifest.json');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  const service = manifest.services?.[options.service];
  if (!service || typeof service !== 'object' || Array.isArray(service)) {
    throw new Error(`service is not registered in the native contract manifest: ${options.service}`);
  }
  assertCanonicalRoutes(options.service, service.docsRoutes);
  const publicContractPath = assertRepoPath(repoRoot, service.publicContract);
  const expectedBytes = await readFile(publicContractPath);
  const expectedDocument = JSON.parse(expectedBytes.toString('utf8'));
  assertPublicContract(options.service, expectedDocument);

  const baseUrl = canonicalBaseUrl(options.baseUrl);
  const report = {
    schemaVersion: 1,
    service: options.service,
    baseUrl: baseUrl.toString(),
    publicContract: service.publicContract,
    publicContractBytes: expectedBytes.length,
    publicContractSha256: sha256(expectedBytes),
    routes: [],
  };

  for (const route of CANONICAL_DOC_ROUTES) {
    let live;
    try {
      live = await fetchRoute(baseUrl, route, options);
      if (JSON_DOC_ROUTES.has(route)) {
        if (!jsonContentType(live.contentType)) {
          throw new Error(
            `GET ${route} returned non-JSON content type ${JSON.stringify(live.contentType)}`,
          );
        }
        if (!live.body.equals(expectedBytes)) {
          throw new Error(
            `GET ${route} bytes differ from ${service.publicContract}: expected sha256 ${sha256(expectedBytes)}, received ${sha256(live.body)}`,
          );
        }
      } else if (HTML_DOC_ROUTES.has(route)) {
        if (!htmlContentType(live.contentType)) {
          throw new Error(
            `GET ${route} returned non-HTML content type ${JSON.stringify(live.contentType)}`,
          );
        }
        assertHtmlBootstrap(options.service, route, live.body, service.internalDocsRoutes);
      }
      report.routes.push({
        route,
        status: live.status,
        contentType: live.contentType,
        bytes: live.body.length,
        sha256: sha256(live.body),
      });
    } catch (error) {
      throw new Error(`${options.service} live conformance failed: ${error.message}`);
    }
  }

  if (options.json) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    return;
  }
  process.stdout.write(
    [
      `${options.service}: live public API contract conformance passed`,
      `base URL: ${report.baseUrl}`,
      `contract: ${report.publicContract}`,
      `sha256: ${report.publicContractSha256}`,
      ...report.routes.map(
        (route) =>
          `${route.status} ${route.route} ${route.contentType || '(no content type)'} ${route.bytes} bytes`,
      ),
    ].join('\n') + '\n',
  );
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error.message ?? String(error)}\n`);
  process.exitCode = 1;
});
