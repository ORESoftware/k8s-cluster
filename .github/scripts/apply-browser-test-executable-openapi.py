#!/usr/bin/env python3

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, content: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def replace_once(path: str, old: str, new: str, label: str) -> None:
    source = read(path)
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    write(path, source.replace(old, new, 1))


def replace_between(path: str, start: str, end: str, replacement: str, label: str) -> None:
    source = read(path)
    start_index = source.find(start)
    if start_index < 0:
        raise SystemExit(f"{path}: {label}: missing start marker")
    end_index = source.find(end, start_index)
    if end_index < 0:
        raise SystemExit(f"{path}: {label}: missing end marker")
    write(path, source[:start_index] + replacement + source[end_index:])


SERVER = "remote/deployments/browser-test-server/src/server.ts"
CONTRACT = "remote/deployments/browser-test-server/src/api-contract.ts"
PACKAGE = "remote/deployments/browser-test-server/package.json"
GENERATOR = "remote/tools/generate-api-docs.mjs"
MANIFEST = "remote/api-contracts/manifest.json"
POLICY = "remote/config/api-contracts.json"

replace_once(
    SERVER,
    """import Fastify from 'fastify';
import { initTelemetry, instrumentFastify, loggerMixin } from '@dd/telemetry';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { z } from 'zod';
""",
    """import swagger from '@fastify/swagger';
import {
  registerAjvFormats,
  type TypeBoxTypeProvider,
  TypeBoxValidatorCompiler,
} from '@fastify/type-provider-typebox';
import Fastify, { type FastifyReply } from 'fastify';
import { initTelemetry, instrumentFastify, loggerMixin } from '@dd/telemetry';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
""",
    "replace imports",
)

replace_once(
    SERVER,
    """import { Options as ChromeOptions } from 'selenium-webdriver/chrome.js';
""",
    """import { Options as ChromeOptions } from 'selenium-webdriver/chrome.js';
import {
  canonicalJson,
  finalizeInternalOpenApiDocument,
  OPENAPI_SWAGGER_OPTIONS,
  routeSchemas,
  scalarApiReferenceHtml,
  TOOLS,
  type ConsoleLogEntry,
  type RunRequest,
  type RunResult,
  type ScreenshotPayload,
  type Step,
  type StepLogEntry,
  type Tool,
} from './api-contract.js';
import { PUBLIC_OPENAPI_JSON } from './generated/public-openapi.js';
""",
    "add contract imports",
)

replace_once(
    SERVER,
    """type Tool = 'playwright' | 'puppeteer' | 'selenium';

const TOOLS = ['playwright', 'puppeteer', 'selenium'] as const;

""",
    "",
    "remove duplicate tool type",
)

replace_between(
    SERVER,
    "const StepBaseSchema = z.object({",
    "const metrics = {",
    "const metrics = {",
    "remove parallel Zod contract",
)

server_setup = r"""const exportingOpenApi = process.argv.includes('--export-openapi');
const telemetry = exportingOpenApi
  ? { shutdown: async () => {} }
  : initTelemetry('dd-browser-test-server');

registerAjvFormats();

const fastify = Fastify({
  logger: exportingOpenApi ? false : { mixin: loggerMixin },
  bodyLimit: 2_097_152,
})
  .setValidatorCompiler(TypeBoxValidatorCompiler)
  .withTypeProvider<TypeBoxTypeProvider>();

if (!exportingOpenApi) {
  instrumentFastify(fastify, { service: 'dd-browser-test-server' });
}

await fastify.register(swagger, OPENAPI_SWAGGER_OPTIONS);

fastify.setErrorHandler((error, request, reply) => {
  if (error.validation) {
    return reply.code(400).send({
      ok: false,
      error: 'request validation failed',
      details: error.validation,
    });
  }
  request.log.error({ err: error }, 'browser-test request failed');
  const statusCode =
    typeof error.statusCode === 'number' && error.statusCode >= 400
      ? error.statusCode
      : 500;
  return reply.code(statusCode).send({
    ok: false,
    error: statusCode >= 500 ? 'internal server error' : error.message,
  });
});

fastify.addHook('onRequest', async (request, reply) => {
  const visibility = (
    request.routeOptions.schema as { 'x-dd-visibility'?: unknown } | undefined
  )?.['x-dd-visibility'];
  if (visibility !== 'internal') return;
  if (isAuthorized(request.headers)) return;
  return reply.code(401).send({ ok: false, error: 'unauthorized' });
});

let internalOpenApiDocumentCache: Record<string, unknown> | null = null;
let internalOpenApiJsonCache: string | null = null;
let internalScalarHtmlCache: string | null = null;
const publicScalarHtml = scalarApiReferenceHtml(
  '/openapi.json',
  'Browser Test Server Public API',
);

function internalOpenApiDocument(): Record<string, unknown> {
  if (!internalOpenApiDocumentCache) {
    internalOpenApiDocumentCache = finalizeInternalOpenApiDocument(fastify.swagger());
  }
  return internalOpenApiDocumentCache;
}

function internalOpenApiJson(): string {
  if (!internalOpenApiJsonCache) {
    internalOpenApiJsonCache = canonicalJson(internalOpenApiDocument());
  }
  return internalOpenApiJsonCache;
}

function internalScalarHtml(): string {
  if (!internalScalarHtmlCache) {
    internalScalarHtmlCache = scalarApiReferenceHtml(
      internalOpenApiDocument(),
      'Browser Test Server Internal API',
    );
  }
  return internalScalarHtmlCache;
}

function sendExactPayload(
  reply: FastifyReply,
  contentType: string,
  payload: string,
): FastifyReply {
  reply.hijack();
  reply.raw.statusCode = 200;
  reply.raw.setHeader('content-type', contentType);
  reply.raw.end(payload);
  return reply;
}

fastify.get('/', { schema: routeSchemas.root }, async () => serviceDescriptor());
fastify.get(
  '/browser-test',
  { schema: routeSchemas.browserTestRoot },
  async () => serviceDescriptor(),
);
fastify.get('/tools', { schema: routeSchemas.tools }, async () => toolsDescriptor());
fastify.get(
  '/browser-test/tools',
  { schema: routeSchemas.browserTestTools },
  async () => toolsDescriptor(),
);
fastify.get('/status', { schema: routeSchemas.status }, async () => statusDescriptor());
fastify.get(
  '/browser-test/status',
  { schema: routeSchemas.browserTestStatus },
  async () => statusDescriptor(),
);
fastify.get('/healthz', { schema: routeSchemas.healthz }, async () => healthDescriptor());
fastify.get(
  '/browser-test/healthz',
  { schema: routeSchemas.browserTestHealthz },
  async () => healthDescriptor(),
);
fastify.get('/metrics', { schema: routeSchemas.metrics }, async (_request, reply) => {
  reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  return renderMetrics();
});
fastify.get(
  '/browser-test/metrics',
  { schema: routeSchemas.browserTestMetrics },
  async (_request, reply) => {
    reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
    return renderMetrics();
  },
);
fastify.get(
  '/openapi.json',
  { schema: routeSchemas.publicOpenApi },
  async (_request, reply) =>
    sendExactPayload(reply, 'application/json; charset=utf-8', PUBLIC_OPENAPI_JSON),
);
fastify.get(
  '/api/docs.json',
  { schema: routeSchemas.publicOpenApiAlias },
  async (_request, reply) =>
    sendExactPayload(reply, 'application/json; charset=utf-8', PUBLIC_OPENAPI_JSON),
);
fastify.get('/api/docs', { schema: routeSchemas.publicScalar }, async (_request, reply) => {
  reply.header('content-type', 'text/html; charset=utf-8');
  return publicScalarHtml;
});
fastify.get(
  '/docs/api',
  { schema: routeSchemas.publicScalarAlias },
  async (_request, reply) => {
    reply.header('content-type', 'text/html; charset=utf-8');
    return publicScalarHtml;
  },
);
fastify.get(
  '/internal/openapi.json',
  { schema: routeSchemas.internalOpenApi },
  async (_request, reply) =>
    sendExactPayload(reply, 'application/json; charset=utf-8', internalOpenApiJson()),
);
fastify.get(
  '/internal/docs/api',
  { schema: routeSchemas.internalScalar },
  async (_request, reply) => {
    reply.header('content-type', 'text/html; charset=utf-8');
    return internalScalarHtml();
  },
);

fastify.post<{ Body: RunRequest }>(
  '/run',
  { schema: routeSchemas.run },
  async (request, reply) => {
    const input = request.body;
    if (input.steps.length > config.maxSteps) {
      return reply.code(400).send({
        ok: false,
        error: 'request validation failed',
        details: [{
          instancePath: '/steps',
          keyword: 'maxItems',
          message: `must NOT have more than ${config.maxSteps} items`,
          params: { limit: config.maxSteps },
          schemaPath: '#/properties/steps/maxItems',
        }],
      });
    }
    if (input.timeoutMs !== undefined && input.timeoutMs > config.maxTimeoutMs) {
      return reply.code(400).send({
        ok: false,
        error: 'request validation failed',
        details: [{
          instancePath: '/timeoutMs',
          keyword: 'maximum',
          message: `must be <= ${config.maxTimeoutMs}`,
          params: { comparison: '<=', limit: config.maxTimeoutMs },
          schemaPath: '#/properties/timeoutMs/maximum',
        }],
      });
    }

    if (metrics.inFlight >= config.maxConcurrent) {
      return reply.code(429).send({
        ok: false,
        error: 'browser-test concurrency limit reached',
        maxConcurrent: config.maxConcurrent,
      });
    }

    const tool: Tool = input.tool ?? config.defaultTool;
    const requestId = input.requestId ?? randomUUID();
    const startedAtIso = new Date().toISOString();
    const startedAtMs = Date.now();
    metrics.inFlight += 1;

    try {
      const result = await runScenario(tool, input, requestId, startedAtIso);
      recordMetric(tool, result.ok ? 'ok' : 'error', result.durationMs);
      if (!result.ok) {
        return reply.code(422).send(result);
      }
      return result;
    } catch (error) {
      const durationMs = Date.now() - startedAtMs;
      recordMetric(tool, 'error', durationMs);
      const message = error instanceof Error ? error.message : String(error);
      return reply.code(500).send({
        ok: false,
        requestId,
        tool,
        durationMs,
        startedAt: startedAtIso,
        finishedAt: new Date().toISOString(),
        steps: [],
        extracted: {},
        screenshots: [],
        consoleEntries: [],
        pageErrors: [],
        error: message,
      });
    } finally {
      metrics.inFlight -= 1;
    }
  },
);

"""

replace_between(
    SERVER,
    "const telemetry = initTelemetry('dd-browser-test-server');",
    "async function runScenario(",
    server_setup,
    "replace Fastify HTTP boundary",
)

replace_once(
    SERVER,
    """    endpoints: {
      run: 'POST /run',
      tools: 'GET /browser-test/tools',
      status: 'GET /browser-test/status',
      healthz: 'GET /browser-test/healthz',
      metrics: 'GET /browser-test/metrics',
    },
""",
    """    endpoints: {
      run: 'POST /run',
      tools: 'GET /tools',
      status: 'GET /status',
      healthz: 'GET /healthz',
      metrics: 'GET /metrics',
      publicOpenApi: 'GET /openapi.json',
      internalOpenApi: 'GET /internal/openapi.json',
    },
""",
    "update service descriptor",
)

server_bottom = r"""async function closeResources() {
  try {
    await fastify.close();
  } finally {
    if (playwrightBrowser) {
      try {
        await playwrightBrowser.close();
      } catch {
        // ignore
      }
    }
    if (puppeteerBrowser) {
      try {
        await puppeteerBrowser.close();
      } catch {
        // ignore
      }
    }
    await telemetry.shutdown();
  }
}

async function shutdown(signal: NodeJS.Signals) {
  fastify.log.info({ signal }, 'browser-test shutting down');
  await closeResources();
  process.exitCode = 0;
}

export async function buildServer() {
  await fastify.ready();
  return fastify;
}

async function main() {
  await fastify.ready();
  if (exportingOpenApi) {
    process.stdout.write(internalOpenApiJson());
    await closeResources();
    return;
  }
  const address = await fastify.listen({ host: config.host, port: config.port });
  fastify.log.info({ address }, 'dd-browser-test-server listening');
}

const isMain =
  process.argv[1] !== undefined && pathToFileURL(process.argv[1]).href === import.meta.url;

if (isMain) {
  process.on('SIGTERM', () => void shutdown('SIGTERM'));
  process.on('SIGINT', () => void shutdown('SIGINT'));
  void main().catch(async (error) => {
    fastify.log.error({ err: error }, 'dd-browser-test-server failed to start');
    process.exitCode = 1;
    await closeResources().catch(() => {});
  });
}

export type { RunRequest, RunResult, Step };
"""

replace_between(
    SERVER,
    "async function shutdown(signal: NodeJS.Signals)",
    "export type { RunRequest, RunResult, Step };",
    server_bottom,
    "make startup import-safe and exporter side-effect-free",
)
# replace_between leaves the end marker in place; remove the duplicate export.
replace_once(
    SERVER,
    "export type { RunRequest, RunResult, Step };\nexport type { RunRequest, RunResult, Step };\n",
    "export type { RunRequest, RunResult, Step };\n",
    "deduplicate exported types",
)

scalar_start = "export function scalarApiReferenceHtml(specUrl: string, title: string): string {"
scalar_end = "export type Tool = Static<typeof ToolSchema>;"
scalar_function = r"""export function scalarApiReferenceHtml(
  source: string | Record<string, unknown>,
  title: string,
): string {
  const safeTitle = escapeHtml(title);
  const specUrl = typeof source === 'string' ? source : '/internal/openapi.json';
  const configuration = {
    ...(typeof source === 'string' ? { url: source } : { content: source }),
    pageTitle: title,
    theme: 'default',
    hideClientButton: false,
    persistAuth: false,
  };
  const serializedConfiguration = JSON.stringify(configuration)
    .replaceAll('&', '\\u0026')
    .replaceAll('<', '\\u003c')
    .replaceAll('>', '\\u003e');
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>${safeTitle}</title>
</head>
<body>
  <noscript><p>JavaScript is required for the interactive reference. Open <a href="${escapeHtml(specUrl)}">the OpenAPI JSON</a>.</p></noscript>
  <div id="app"></div>
  <script src="${SCALAR_BROWSER_URL}"></script>
  <script>Scalar.createApiReference('#app', ${serializedConfiguration});</script>
</body>
</html>
`;
}

"""
replace_between(CONTRACT, scalar_start, scalar_end, scalar_function, "embed authenticated Scalar content")

package = json.loads(read(PACKAGE))
package["scripts"] = {
    "build": "tsc",
    "dev": "tsx watch src/server.ts",
    "docs:embed": "node scripts/embed-public-api-docs.mjs",
    "docs:embed:check": "node scripts/embed-public-api-docs.mjs --check",
    "openapi:export": "tsx src/server.ts --export-openapi",
    "start": "node dist/server.js",
    "start:src": "tsx src/server.ts",
    "test": "pnpm build && node --test test/openapi-contract.test.mjs",
    "test:contract": "pnpm test",
    "typecheck": "tsc --noEmit",
}
dependencies = package["dependencies"]
dependencies.pop("zod", None)
dependencies.update(
    {
        "@fastify/swagger": "9.8.1",
        "@fastify/type-provider-typebox": "6.1.0",
        "ajv-formats": "3.0.1",
        "fastify": "5.10.0",
        "typebox": "1.1.14",
    }
)
package["dependencies"] = dict(sorted(dependencies.items()))
package["devDependencies"]["typescript"] = "6.0.2"
package["devDependencies"] = dict(sorted(package["devDependencies"].items()))
write(PACKAGE, json.dumps(package, indent=2) + "\n")

replace_once(
    GENERATOR,
    """function classifyRoute(serviceName, route) {
  if (serviceName === 'rest-api-rs' && route.path.startsWith('/internal/db')) {
""",
    """function classifyRoute(serviceName, route) {
  if (route.routeTypeHint) {
    return route.routeTypeHint;
  }
  if (serviceName === 'rest-api-rs' && route.path.startsWith('/internal/db')) {
""",
    "honor native route type",
)

replace_once(
    GENERATOR,
    """function routeAuth(routeType, route) {
  if (routeType === 'internal-db') {
""",
    """function routeAuth(routeType, route) {
  if (route.authHint) {
    return route.authHint;
  }
  if (routeType === 'internal-db') {
""",
    "honor native auth",
)

replace_once(
    GENERATOR,
    """        notes: 'Executable OpenAPI contract collected from the same typed handler registration as the runtime Axum router.',
""",
    """        authHint:
          typeof operation['x-dd-auth'] === 'string' ? operation['x-dd-auth'] : null,
        implementationHint:
          typeof operation['x-dd-implementation'] === 'string'
            ? operation['x-dd-implementation']
            : null,
        routeTypeHint:
          typeof operation['x-dd-route-type'] === 'string'
            ? operation['x-dd-route-type']
            : null,
        notes:
          operation['x-dd-implementation'] === 'fastify-typebox'
            ? 'Executable OpenAPI contract collected from the same Fastify TypeBox schema that validates and types the runtime handler.'
            : 'Executable OpenAPI contract collected from the same typed handler registration as the runtime Axum router.',
""",
    "preserve executable OpenAPI metadata",
)

replace_once(
    GENERATOR,
    """    if (route.notes && !current.notes) {
      current.notes = route.notes;
    }
    byPath.set(key, current);
""",
    """    if (route.notes && !current.notes) {
      current.notes = route.notes;
    }
    for (const hint of ['authHint', 'implementationHint', 'routeTypeHint']) {
      if (route[hint] && current[hint] && route[hint] !== current[hint]) {
        throw new Error(`conflicting ${hint} for ${route.path}`);
      }
      if (route[hint] && !current[hint]) {
        current[hint] = route[hint];
      }
    }
    byPath.set(key, current);
""",
    "merge native metadata safely",
)

replace_once(
    GENERATOR,
    """      implementation: route.sourceFiles.some((file) => file.endsWith('/generated/openapi.json'))
        ? 'openapi-code-first'
        : routeType === 'internal-db'
""",
    """      implementation: route.implementationHint ?? (route.sourceFiles.some((file) => file.endsWith('/generated/openapi.json'))
        ? 'openapi-code-first'
        : routeType === 'internal-db'
""",
    "use native implementation hint",
)
replace_once(
    GENERATOR,
    """            : 'code-first',
      auth: routeAuth(routeType, route),
""",
    """            : 'code-first'),
      auth: routeAuth(routeType, route),
""",
    "close implementation fallback",
)

replace_once(
    GENERATOR,
    """      parser: extractNodeRoutes,
      deploymentDir: 'remote/deployments/browser-test-server',
""",
    """      parser: extractNodeRoutes,
      openapiFile: 'remote/deployments/browser-test-server/generated/openapi.json',
      deploymentDir: 'remote/deployments/browser-test-server',
""",
    "opt browser-test into native OpenAPI",
)

replace_once(
    GENERATOR,
    """    const rawRoutes = [];
    for (const file of files) {
      if (await pathExists(file)) {
        rawRoutes.push(...spec.parser(await readUtf8(file), file));
      }
    }
    const deploymentDir = resolve(repoRoot, spec.deploymentDir ?? dirname(dirname(files[0])));
""",
    """    const openapiFile = spec.openapiFile ? resolve(repoRoot, spec.openapiFile) : null;
    const canonicalOpenApi =
      openapiFile && (await pathExists(openapiFile))
        ? JSON.parse(await readUtf8(openapiFile))
        : null;
    const rawRoutes = [];
    if (canonicalOpenApi) {
      rawRoutes.push(...extractOpenApiRoutes(canonicalOpenApi, openapiFile));
    } else {
      for (const file of files) {
        if (await pathExists(file)) {
          rawRoutes.push(...spec.parser(await readUtf8(file), file));
        }
      }
    }
    const deploymentDir = resolve(repoRoot, spec.deploymentDir ?? dirname(dirname(files[0])));
""",
    "prefer opted-in native OpenAPI",
)

replace_once(
    GENERATOR,
    """      outputName: spec.outputName ?? 'api-docs',
      routes: normalizeRoutes(spec.service, rawRoutes),
""",
    """      outputName: spec.outputName ?? 'api-docs',
      canonicalOpenApi,
      routes: normalizeRoutes(spec.service, rawRoutes),
""",
    "carry canonical OpenAPI into artifact generation",
)

replace_once(
    GENERATOR,
    """    const internalOpenapi = buildOpenApi(docs);
""",
    """    const internalOpenapi = service.canonicalOpenApi
      ? structuredClone(service.canonicalOpenApi)
      : buildOpenApi(docs);
""",
    "keep native internal contract exact",
)

manifest = json.loads(read(MANIFEST))
manifest["services"]["browser-test-server"] = {
    "contract": "remote/deployments/browser-test-server/generated/openapi.json",
    "directory": "remote/deployments/browser-test-server",
    "docsRoutes": ["/openapi.json", "/api/docs.json", "/api/docs", "/docs/api"],
    "export": [
        "pnpm",
        "--dir",
        "remote/deployments/browser-test-server",
        "exec",
        "tsx",
        "src/server.ts",
        "--export-openapi",
    ],
    "implementation": "fastify-typebox",
    "language": "typescript",
    "sdk": {
        "dart": {"generator": "dart", "packageName": "browser_test_client"},
        "rust": {"generator": "rust", "packageName": "browser_test_client"},
        "sourceOfTruthRepository": "ORESoftware/k8s-libs-and-shared-defs",
        "typescript": {
            "generator": "typescript-fetch",
            "packageName": "@oresoftware/browser-test-client",
        },
    },
    "visibility": "private",
    "publicContract": "remote/deployments/browser-test-server/generated/api-docs.json",
    "internalDocsRoutes": ["/internal/openapi.json", "/internal/docs/api"],
    "runtimeContractPolicy": "Unauthenticated standard documentation routes serve only the fail-closed public contract; the complete Fastify TypeBox contract and Scalar reference require bearer authentication.",
}
manifest["services"] = dict(sorted(manifest["services"].items()))
write(MANIFEST, json.dumps(manifest, indent=2) + "\n")

policy = json.loads(read(POLICY))
policy["legacySourceScannerAllowlist"] = [
    service for service in policy["legacySourceScannerAllowlist"] if service != "browser-test-server"
]
write(POLICY, json.dumps(policy, indent=2) + "\n")

print("applied browser-test executable OpenAPI migration")
