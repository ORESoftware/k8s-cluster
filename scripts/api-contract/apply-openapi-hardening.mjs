#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..', '..', '..');
const helperCode = "const OPENAPI_METHODS = new Set(['GET', 'PUT', 'POST', 'DELETE', 'OPTIONS', 'HEAD', 'PATCH', 'TRACE']);\n\nfunction openApiPathFromSource(sourcePath) {\n  const pathOnly = sourcePath.split('?', 1)[0] || '/';\n  let wildcardIndex = 0;\n  const normalized = pathOnly\n    .replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, '{$1}')\n    .replace(/<([a-zA-Z_][a-zA-Z0-9_]*)>/g, '{$1}')\n    .replace(/\\*/g, () => {\n      const suffix = wildcardIndex === 0 ? '' : String(wildcardIndex + 1);\n      wildcardIndex += 1;\n      return `{wildcard${suffix}}`;\n    });\n  return normalized.startsWith('/') ? normalized : `/${normalized}`;\n}\n\nfunction openApiPathParameters(path) {\n  return [...path.matchAll(/\\{([^}]+)\\}/g)].map((match) => ({\n    name: match[1],\n    in: 'path',\n    required: true,\n    schema: { type: 'string' },\n  }));\n}\n\nfunction openApiOperationId(service, route, method) {\n  const handler = route.handlers?.[0] ?? route.path;\n  const token = `${service}_${method}_${handler}`\n    .replace(/[^a-zA-Z0-9]+/g, '_')\n    .replace(/^_+|_+$/g, '')\n    .toLowerCase();\n  return token || 'operation';\n}\n\nfunction openApiVisibility(route) {\n  if (route.routeType === 'internal-db' || route.routeType === 'runtime-config') {\n    return 'internal';\n  }\n  return route.auth === 'public' ? 'public' : 'internal';\n}\n\nfunction openApiSecurity(route) {\n  if (openApiVisibility(route) === 'public') {\n    return [];\n  }\n  if (route.auth.includes('X-Server-Auth')) {\n    return [{ serverAuth: [] }];\n  }\n  if (route.auth.includes('operator secret')) {\n    return [{ operatorSecret: [] }];\n  }\n  if (route.auth.includes('webhook signature')) {\n    return [{ webhookSignature: [] }];\n  }\n  return undefined;\n}\n\nfunction buildOpenApi(docs) {\n  const paths = {};\n  const tags = new Set();\n  let operationCount = 0;\n  for (const route of docs.routes) {\n    const path = openApiPathFromSource(route.path);\n    const pathItem = paths[path] ?? {};\n    tags.add(route.routeType);\n    for (const method of route.methods) {\n      if (!OPENAPI_METHODS.has(method)) {\n        continue;\n      }\n      const security = openApiSecurity(route);\n      const operation = {\n        operationId: openApiOperationId(docs.service, route, method),\n        summary: route.purpose,\n        description: route.notes || route.purpose,\n        tags: [route.routeType],\n        parameters: openApiPathParameters(path),\n        responses: {\n          default: {\n            description: 'Response produced by the registered service handler.',\n          },\n        },\n        'x-dd-auth': route.auth,\n        'x-dd-handlers': route.handlers,\n        'x-dd-implementation': route.implementation,\n        'x-dd-route-type': route.routeType,\n        'x-dd-source-files': route.sourceFiles,\n        'x-dd-source-path': route.path,\n        'x-dd-visibility': openApiVisibility(route),\n      };\n      if (security !== undefined) {\n        operation.security = security;\n      }\n      pathItem[method.toLowerCase()] = operation;\n      operationCount += 1;\n    }\n    paths[path] = pathItem;\n  }\n\n  return {\n    openapi: '3.1.0',\n    jsonSchemaDialect: 'https://json-schema.org/draft/2020-12/schema',\n    info: {\n      title: `${docs.service} API`,\n      version: '0.1.0',\n      description:\n        'Generated from the service route registrations. Request and response schemas become authoritative as this service migrates to its native typed OpenAPI adapter.',\n    },\n    tags: [...tags].sort().map((name) => ({ name })),\n    paths,\n    components: {\n      securitySchemes: {\n        serverAuth: {\n          type: 'apiKey',\n          in: 'header',\n          name: 'X-Server-Auth',\n        },\n        operatorSecret: {\n          type: 'apiKey',\n          in: 'header',\n          name: 'X-Operator-Secret',\n        },\n        webhookSignature: {\n          type: 'apiKey',\n          in: 'header',\n          name: 'X-Webhook-Signature',\n        },\n      },\n    },\n    'x-dd-contract-scope': 'internal',\n    'x-dd-generated-by': docs.generatedBy,\n    'x-dd-language': docs.language,\n    'x-dd-operation-count': operationCount,\n    'x-dd-route-count': docs.routeCount,\n    'x-dd-service': docs.service,\n    'x-dd-standard-docs-routes': docs.standardDocsRoutes,\n  };\n}\n\nfunction buildPublicOpenApi(openapi) {\n  const document = structuredClone(openapi);\n  for (const [path, pathItem] of Object.entries(document.paths)) {\n    for (const method of [...OPENAPI_METHODS].map((value) => value.toLowerCase())) {\n      if (pathItem[method]?.['x-dd-visibility'] !== 'public') {\n        delete pathItem[method];\n      }\n    }\n    if (Object.keys(pathItem).length === 0) {\n      delete document.paths[path];\n    }\n  }\n  document.info.title = `${document.info.title} (public)`;\n  document.info.description =\n    'Fail-closed public subset. Only operations explicitly marked public are included.';\n  document['x-dd-contract-scope'] = 'public';\n  document['x-dd-operation-count'] = Object.values(document.paths).reduce(\n    (count, pathItem) =>\n      count +\n      [...OPENAPI_METHODS]\n        .map((value) => value.toLowerCase())\n        .filter((method) => pathItem[method])\n        .length,\n    0,\n  );\n  return document;\n}";

async function read(path) {
  return readFile(resolve(repoRoot, path), 'utf8');
}

async function write(path, content) {
  await writeFile(resolve(repoRoot, path), content);
}

function replaceOnce(source, before, after, label) {
  if (source.includes(after)) {
    return source;
  }
  const first = source.indexOf(before);
  if (first === -1) {
    throw new Error(`cannot patch ${label}: expected source block was not found`);
  }
  if (source.indexOf(before, first + before.length) !== -1) {
    throw new Error(`cannot patch ${label}: expected source block is not unique`);
  }
  return `${source.slice(0, first)}${after}${source.slice(first + before.length)}`;
}

async function patchGenerator() {
  const path = 'remote/tools/generate-api-docs.mjs';
  let source = await read(path);
  const marker = 'const OPENAPI_METHODS = new Set(';
  if (!source.includes(marker)) {
    source = replaceOnce(
      source,
      'function buildDocs(service) {',
      `${helperCode}\n\nfunction buildDocs(service) {`,
      'OpenAPI builders',
    );
  }

  source = replaceOnce(
    source,
    'function gleamApiDocsModule(docs) {',
    'function gleamApiDocsModule(docs, openapi) {',
    'Gleam API docs module signature',
  );
  source = replaceOnce(
    source,
    'const api_docs_json = ${gleamString(`${JSON.stringify(docs, null, 2)}\\n`)}',
    'const api_docs_json = ${gleamString(`${JSON.stringify(openapi, null, 2)}\\n`)}',
    'Gleam OpenAPI payload',
  );

  const beforeMain = `    const docs = buildDocs(service);
    const outputBase = service.outputName ?? 'api-docs';
    const generatedDir = join(service.deploymentDir, 'generated');
    const json = \`\${JSON.stringify(docs, null, 2)}\\n\`;
    const html = renderDocsHtml(docs);
    const generated = [
      relative(repoRoot, join(generatedDir, \`\${outputBase}.json\`)).split(sep).join('/'),
      relative(repoRoot, join(generatedDir, \`\${outputBase}.html\`)).split(sep).join('/'),
    ];
    await writeOrCheck(join(generatedDir, \`\${outputBase}.json\`), json);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.html\`), html);
    if (service.language === 'gleam' && outputBase === 'api-docs' && service.moduleDir) {
      await writeOrCheck(join(service.moduleDir, 'api_docs.gleam'), gleamApiDocsModule(docs));
    }`;

  const afterMain = `    const docs = buildDocs(service);
    const openapi = buildOpenApi(docs);
    const publicOpenapi = buildPublicOpenApi(openapi);
    const outputBase = service.outputName ?? 'api-docs';
    const generatedDir = join(service.deploymentDir, 'generated');
    const json = \`\${JSON.stringify(openapi, null, 2)}\\n\`;
    const publicJson = \`\${JSON.stringify(publicOpenapi, null, 2)}\\n\`;
    const metadataJson = \`\${JSON.stringify(docs, null, 2)}\\n\`;
    const html = renderDocsHtml(docs);
    const generated = [
      relative(repoRoot, join(generatedDir, \`\${outputBase}.json\`)).split(sep).join('/'),
      relative(repoRoot, join(generatedDir, \`\${outputBase}.html\`)).split(sep).join('/'),
      relative(repoRoot, join(generatedDir, \`\${outputBase}.public.json\`)).split(sep).join('/'),
      relative(repoRoot, join(generatedDir, \`\${outputBase}.metadata.json\`)).split(sep).join('/'),
    ];
    await writeOrCheck(join(generatedDir, \`\${outputBase}.json\`), json);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.public.json\`), publicJson);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.metadata.json\`), metadataJson);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.html\`), html);
    if (service.language === 'gleam' && outputBase === 'api-docs' && service.moduleDir) {
      await writeOrCheck(
        join(service.moduleDir, 'api_docs.gleam'),
        gleamApiDocsModule(docs, openapi),
      );
    }`;

  source = replaceOnce(source, beforeMain, afterMain, 'OpenAPI artifact generation');
  await write(path, source);
}

async function patchPackageScripts() {
  const path = 'remote/tests/package.json';
  let source = await read(path);
  source = replaceOnce(
    source,
    '    "check:api-docs": "node ../tools/generate-api-docs.mjs --check",\n',
    '    "check:api-docs": "node ../tools/generate-api-docs.mjs --check",\n    "check:openapi-contracts": "node ../tools/validate-openapi-contracts.mjs",\n',
    'remote test package scripts',
  );
  await write(path, source);
}

async function patchRepoChecks() {
  const path = '.github/workflows/repo-checks.yml';
  let source = await read(path);
  const before = `      - name: Verify generated API docs are current
        run: pnpm run check:api-docs
        working-directory: remote/tests

`;
  const after = `${before}      - name: Validate OpenAPI and public-contract parity
        run: pnpm run check:openapi-contracts
        working-directory: remote/tests

`;
  source = replaceOnce(source, before, after, 'repo checks OpenAPI validation step');
  await write(path, source);
}

async function patchAgents() {
  const path = 'AGENTS.md';
  let source = await read(path);
  const before = `HTTP API deployments should expose generated API docs at \`/docs/api\` and \`/api/docs\`, with
machine-readable metadata at \`/api/docs.json\`. Docs must be derived from route declarations or
equivalent runtime source using \`remote/tools/generate-api-docs.mjs\`; do not maintain manual route
inventories for API docs. Non-Rust runtimes may use runtime-specific generated artifacts or modules,
but they should still come from source scanning and be checked with \`--check\` in CI.`;
  const after = `HTTP API deployments must expose human-readable docs at \`/docs/api\` and \`/api/docs\`,
and a valid OpenAPI 3.1 document at \`/api/docs.json\`. Route registration, runtime validation,
request/response schemas, OpenAPI generation, and SDK generation must share one typed source of
truth. Rust uses \`utoipa\` plus \`utoipa-axum\` route registration; Node/Fastify uses route schemas
consumed by \`@fastify/swagger\`; Gleam and Dart use typed route registries that drive both dispatch
and OpenAPI output. The source scanner in \`remote/tools/generate-api-docs.mjs\` is a temporary,
explicitly allowlisted migration bridge only. New services must use a native strategy, and CI must
regenerate/check both the full and fail-closed public OpenAPI artifacts before SDK publication.
See \`docs/http-api-openapi-sdk-contract.md\`.`;
  source = replaceOnce(source, before, after, 'AGENTS API docs contract');
  await write(path, source);
}

await patchGenerator();
await patchPackageScripts();
await patchRepoChecks();
await patchAgents();
console.log('applied OpenAPI contract hardening patch');
