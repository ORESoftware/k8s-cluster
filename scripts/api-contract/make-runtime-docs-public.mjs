#!/usr/bin/env node

import { readdir, readFile, unlink, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..', '..');

async function read(path) {
  return readFile(resolve(repoRoot, path), 'utf8');
}

async function write(path, content) {
  await writeFile(resolve(repoRoot, path), content);
}

function replaceOnce(source, pattern, replacement, label) {
  const flags = pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`;
  const matches = source.match(new RegExp(pattern.source, flags));
  if (matches?.length !== 1) {
    throw new Error(`${label}: expected exactly one source match, found ${matches?.length ?? 0}`);
  }
  return source.replace(pattern, replacement);
}

async function removeLegacyPublicAliases(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await removeLegacyPublicAliases(path);
    } else if (entry.isFile() && entry.name.endsWith('.public.json')) {
      await unlink(path);
    }
  }
}

async function patchGenerator() {
  const path = 'remote/tools/generate-api-docs.mjs';
  let source = await read(path);

  const publicDocsBuilder = `function buildPublicDocs(docs) {
  const routes = docs.routes.filter((route) => openApiVisibility(route) === 'public');
  const routeTypeCounts = routes.reduce((acc, route) => {
    acc[route.routeType] = (acc[route.routeType] ?? 0) + 1;
    return acc;
  }, {});
  return {
    ...docs,
    routeCount: routes.length,
    routeTypeCounts,
    routes,
    contractScope: 'public',
  };
}

function buildDocs`;
  if (!source.includes('function buildPublicDocs(docs) {')) {
    source = replaceOnce(
      source,
      /function buildDocs/,
      publicDocsBuilder,
      'insert public docs builder',
    );
  }

  source = replaceOnce(
    source,
    /function renderDocsIndexHtml\(items\) \{[\s\S]*?\n\}\n\nfunction gleamString/,
    `function renderDocsIndexHtml(services) {
  const totalRoutes = services.reduce((sum, service) => sum + service.routeCount, 0);
  const serviceRows = services
    .map((service) => {
      const publicJson = service.generated[0];
      const publicHtml = service.generated[1];
      return \`<tr>
  <td data-label="Service"><code>\${escapeHtml(service.service)}</code></td>
  <td data-label="Language">\${escapeHtml(service.language)}</td>
  <td data-label="Routes">\${service.routeCount}</td>
  <td data-label="Public docs"><code>\${escapeHtml(publicHtml)}</code></td>
  <td data-label="Public OpenAPI"><code>\${escapeHtml(publicJson)}</code></td>
</tr>\`;
    })
    .join('\\n');
  return \`<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>dd runtime API docs</title>
  <style>
    :root { color-scheme: light; --bg:#f7f8fa; --panel:#fff; --ink:#17202a; --muted:#5b6672; --line:#d8dee6; --code:#eef2f6; }
    * { box-sizing:border-box; }
    body { margin:0; background:var(--bg); color:var(--ink); font:14px/1.5 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; }
    header,main { width:min(1180px,calc(100% - 32px)); margin:0 auto; }
    header { padding:28px 0 18px; }
    h1 { margin:0 0 6px; font-size:30px; line-height:1.15; }
    p { margin:0; color:var(--muted); }
    .summary { display:flex; flex-wrap:wrap; gap:10px; margin-top:18px; }
    .summary span { display:inline-flex; align-items:center; min-height:26px; border:1px solid var(--line); border-radius:6px; padding:3px 9px; background:var(--panel); }
    table { width:100%; border-collapse:collapse; background:var(--panel); border:1px solid var(--line); border-radius:8px; overflow:hidden; }
    th,td { padding:11px 12px; border-bottom:1px solid var(--line); vertical-align:top; text-align:left; }
    th { color:var(--muted); font-size:12px; text-transform:uppercase; background:#fbfcfd; }
    tr:last-child td { border-bottom:0; }
    code { display:inline-block; max-width:100%; padding:2px 5px; border-radius:5px; background:var(--code); overflow-wrap:anywhere; font-family:ui-monospace,"SFMono-Regular",Consolas,monospace; font-size:12px; }
    @media (max-width:760px) {
      header,main { width:min(100% - 20px,1180px); }
      table,tbody,tr,td { display:block; width:100%; }
      thead { display:none; }
      tr { border-bottom:1px solid var(--line); }
      td { border-bottom:0; padding:8px 10px; }
      td::before { display:block; margin-bottom:3px; color:var(--muted); font-size:11px; font-weight:700; text-transform:uppercase; content:attr(data-label); }
    }
  </style>
</head>
<body>
  <header>
    <h1>dd runtime API docs</h1>
    <p>Public-only fleet index. Internal contracts are unserved build artifacts used for private SDK generation and parity checks.</p>
    <div class="summary">
      <span>\${services.length} services</span>
      <span>\${totalRoutes} registered routes</span>
      <span>central JSON <code>/api-docs.json</code></span>
    </div>
  </header>
  <main>
    <table>
      <thead><tr><th>Service</th><th>Language</th><th>Registered routes</th><th>Public docs</th><th>Public OpenAPI</th></tr></thead>
      <tbody>
\${serviceRows}
      </tbody>
    </table>
  </main>
</body>
</html>
\`;
}

function gleamString`,
    'replace central docs HTML renderer',
  );

  source = replaceOnce(
    source,
    /function canonicalGeneratedArtifacts\(openapiPath\) \{[\s\S]*?\n\}/,
    `function canonicalGeneratedArtifacts(publicOpenapiPath) {
  if (
    typeof publicOpenapiPath !== 'string' ||
    !publicOpenapiPath.endsWith('.json') ||
    publicOpenapiPath.endsWith('.internal.json') ||
    publicOpenapiPath.endsWith('.metadata.json')
  ) {
    throw new Error(\`invalid public OpenAPI artifact path: \${publicOpenapiPath}\`);
  }
  return [
    publicOpenapiPath,
    publicOpenapiPath.replace(/\\.json$/, '.html'),
    publicOpenapiPath.replace(/\\.json$/, '.internal.json'),
    publicOpenapiPath.replace(/\\.json$/, '.metadata.json'),
  ];
}`,
    'replace canonical artifact layout',
  );

  source = source.replace(
    "      !path.endsWith('.public.json') &&\n      !path.endsWith('.metadata.json'),",
    "      !path.endsWith('.internal.json') &&\n      !path.endsWith('.metadata.json'),",
  );
  source = source.replace(
    "throw new Error(`${service.service ?? 'unknown service'} has no canonical OpenAPI JSON artifact`);",
    "throw new Error(`${service.service ?? 'unknown service'} has no public OpenAPI JSON artifact`);",
  );

  source = replaceOnce(
    source,
    /    const docs = buildDocs\(service\);[\s\S]*?    indexItems\.push\(\{ docs, generated \}\);/,
    `    const docs = buildDocs(service);
    const internalOpenapi = buildOpenApi(docs);
    const publicOpenapi = buildPublicOpenApi(internalOpenapi);
    const publicDocs = buildPublicDocs(docs);
    const outputBase = service.outputName ?? 'api-docs';
    const generatedDir = join(service.deploymentDir, 'generated');
    const publicJson = \`\${JSON.stringify(publicOpenapi, null, 2)}\\n\`;
    const internalJson = \`\${JSON.stringify(internalOpenapi, null, 2)}\\n\`;
    const metadataJson = \`\${JSON.stringify(docs, null, 2)}\\n\`;
    const html = renderDocsHtml(publicDocs);
    const publicOpenapiPath = relative(
      repoRoot,
      join(generatedDir, \`\${outputBase}.json\`),
    ).split(sep).join('/');
    const generated = canonicalGeneratedArtifacts(publicOpenapiPath);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.json\`), publicJson);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.internal.json\`), internalJson);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.metadata.json\`), metadataJson);
    await writeOrCheck(join(generatedDir, \`\${outputBase}.html\`), html);
    if (service.language === 'gleam' && outputBase === 'api-docs' && service.moduleDir) {
      await writeOrCheck(
        join(service.moduleDir, 'api_docs.gleam'),
        gleamApiDocsModule(publicDocs, publicOpenapi),
      );
    }
    index.push({
      service: service.service,
      language: service.language,
      routeCount: docs.routeCount,
      routeTypeCounts: docs.routeTypeCounts,
      generated,
    });
    indexItems.push({ docs: publicDocs, generated });`,
    'replace service artifact generation',
  );

  source = source.replace(
    "  document['x-dd-operation-count'] = Object.values(document.paths).reduce(\n",
    "  document['x-dd-route-count'] = new Set(\n    operationEntriesForDocument(document).flatMap((entry) => entry.operation['x-dd-source-paths'] ?? []),\n  ).size;\n  document['x-dd-operation-count'] = Object.values(document.paths).reduce(\n",
  );

  if (!source.includes('function operationEntriesForDocument(document) {')) {
    const helper = `function operationEntriesForDocument(document) {
  const entries = [];
  for (const pathItem of Object.values(document.paths ?? {})) {
    for (const method of [...OPENAPI_METHODS].map((value) => value.toLowerCase())) {
      if (pathItem[method]) {
        entries.push({ method, operation: pathItem[method] });
      }
    }
  }
  return entries;
}

function buildPublicOpenApi`;
    source = replaceOnce(
      source,
      /function buildPublicOpenApi/,
      helper,
      'insert public route count helper',
    );
  }

  const partialWriteAnchor = `      await writeOrCheck(
        centralIndexJson,
        \`\${JSON.stringify(mergedPayload, null, 2)}\\n\`,
      );
      console.log(`;
  if (!source.includes('renderDocsIndexHtml(mergedServices)')) {
    if (!source.includes(partialWriteAnchor)) {
      throw new Error('partial central index write anchor was not found');
    }
    source = source.replace(
      partialWriteAnchor,
      `      await writeOrCheck(
        centralIndexJson,
        \`\${JSON.stringify(mergedPayload, null, 2)}\\n\`,
      );
      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(mergedServices));
      console.log(`,
    );
  }
  source = source.replace(
    '      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(indexItems));',
    '      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(normalizedPayload.services));',
  );

  await write(path, source);
}

async function patchValidator() {
  const path = 'remote/tools/validate-openapi-contracts.mjs';
  let source = await read(path);

  source = replaceOnce(
    source,
    /function expectedGeneratedArtifacts\(openapiRelative\) \{[\s\S]*?\n\}/,
    `function expectedGeneratedArtifacts(publicRelative) {
  return [
    publicRelative,
    publicRelative.replace(/\\.json$/, '.html'),
    publicRelative.replace(/\\.json$/, '.internal.json'),
    publicRelative.replace(/\\.json$/, '.metadata.json'),
  ];
}`,
    'replace validator artifact layout',
  );

  source = replaceOnce(
    source,
    /function verifyService\(item, gitlinks, fleetOperationIds\) \{[\s\S]*?\n\}\n\nfunction main/,
    `function verifyService(item, gitlinks, fleetOperationIds) {
  const publicRelative = item.generated?.[0];
  assert(
    typeof publicRelative === 'string' &&
      publicRelative.endsWith('.json') &&
      !publicRelative.endsWith('.internal.json') &&
      !publicRelative.endsWith('.metadata.json'),
    \`\${item.service}: central index must identify the public runtime OpenAPI artifact\`,
  );
  assert(
    JSON.stringify(item.generated) === JSON.stringify(expectedGeneratedArtifacts(publicRelative)),
    \`\${item.service}: central index generated artifacts must list public JSON, public HTML, internal JSON, and metadata JSON in canonical order\`,
  );
  if (unavailableGitlink(publicRelative, gitlinks)) {
    return { skipped: true };
  }

  const publicPath = resolve(repoRoot, publicRelative);
  const internalPath = resolve(repoRoot, publicRelative.replace(/\\.json$/, '.internal.json'));
  const metadataPath = resolve(repoRoot, publicRelative.replace(/\\.json$/, '.metadata.json'));
  for (const artifactPath of [publicPath, internalPath, metadataPath]) {
    assert(existsSync(artifactPath), \`\${item.service}: missing \${displayPath(artifactPath)}\`);
  }

  const publicOpenapi = readJson(publicPath);
  const internalOpenapi = readJson(internalPath);
  const metadata = readJson(metadataPath);
  verifyOpenApiShape(publicOpenapi, item.service, publicPath);
  verifyOpenApiShape(internalOpenapi, item.service, internalPath);
  assert(
    publicOpenapi['x-dd-contract-scope'] === 'public',
    \`\${item.service}: runtime OpenAPI must be the public contract\`,
  );
  assert(
    internalOpenapi['x-dd-contract-scope'] === 'internal',
    \`\${item.service}: private SDK artifact must be the internal contract\`,
  );
  assert(metadata.service === item.service, \`\${item.service}: metadata service mismatch\`);
  assert(metadata.language === item.language, \`\${item.service}: metadata language mismatch\`);
  assert(
    metadata.routeCount === metadata.routes?.length,
    \`\${item.service}: metadata routeCount is stale\`,
  );

  const fullEntries = operationEntries(internalOpenapi);
  const actualKeys = fullEntries.flatMap(operationSourceKeys).sort();
  const expectedKeys = expectedRouteKeys(metadata);
  assert(
    JSON.stringify(actualKeys) === JSON.stringify(expectedKeys),
    \`\${item.service}: internal OpenAPI route/method set drifted from generated route metadata\`,
  );

  const localOperationIds = new Set();
  for (const entry of fullEntries) {
    const operationId = entry.operation.operationId;
    assert(
      typeof operationId === 'string' && operationId.length > 0,
      \`\${item.service}: \${entry.method} \${entry.path} has no operationId\`,
    );
    assert(!localOperationIds.has(operationId), \`\${item.service}: duplicate operationId \${operationId}\`);
    assert(!fleetOperationIds.has(operationId), \`fleet duplicate operationId \${operationId}\`);
    localOperationIds.add(operationId);
    fleetOperationIds.add(operationId);
    assert(
      ['public', 'internal'].includes(entry.operation['x-dd-visibility']),
      \`\${item.service}: \${operationId} must declare x-dd-visibility\`,
    );
    assert(
      typeof entry.operation['x-dd-auth'] === 'string',
      \`\${item.service}: \${operationId} must declare x-dd-auth\`,
    );
  }

  const standardRoutes = new Set(metadata.standardDocsRoutes ?? []);
  for (const route of ['/docs/api', '/api/docs', '/api/docs.json']) {
    assert(standardRoutes.has(route), \`\${item.service}: metadata omits standard route \${route}\`);
    assert(
      actualKeys.includes(\`GET \${route}\`),
      \`\${item.service}: internal OpenAPI omits GET \${route}\`,
    );
  }

  const publicEntries = operationEntries(publicOpenapi);
  const fullByKey = new Map(fullEntries.map((entry) => [operationDocumentKey(entry), entry]));
  for (const entry of publicEntries) {
    const key = operationDocumentKey(entry);
    assert(fullByKey.has(key), \`\${item.service}: public OpenAPI contains non-canonical operation \${key}\`);
    assert(
      JSON.stringify(operationSourceKeys(entry)) === JSON.stringify(operationSourceKeys(fullByKey.get(key))),
      \`\${item.service}: public OpenAPI source-path set drifted for \${key}\`,
    );
    assert(
      entry.operation['x-dd-visibility'] === 'public',
      \`\${item.service}: internal operation leaked into runtime OpenAPI: \${key}\`,
    );
  }
  const expectedPublicKeys = fullEntries
    .filter((entry) => entry.operation['x-dd-visibility'] === 'public')
    .map(operationDocumentKey)
    .sort();
  const actualPublicKeys = publicEntries.map(operationDocumentKey).sort();
  assert(
    JSON.stringify(expectedPublicKeys) === JSON.stringify(actualPublicKeys),
    \`\${item.service}: runtime OpenAPI is not the exact public subset\`,
  );
  for (const route of ['/docs/api', '/api/docs', '/api/docs.json']) {
    assert(
      actualPublicKeys.includes(\`GET \${route}\`),
      \`\${item.service}: public runtime OpenAPI omits GET \${route}\`,
    );
  }

  return {
    skipped: false,
    operations: fullEntries.length,
    publicOperations: publicEntries.length,
  };
}

function main`,
    'replace validator service checks',
  );

  await write(path, source);
}

async function patchParityTest() {
  const path = 'remote/tests/check-rest-api-route-parity.mjs';
  const content = `#!/usr/bin/env node
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
  throw new Error(\`Unable to locate repo root from \${process.cwd()}\`);
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
    \`\${script} \${args.join(' ')} failed.\\nSTDOUT:\\n\${result.stdout}\\nSTDERR:\\n\${result.stderr}\`,
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
      .split('\\n')
      .filter(Boolean)
      .map((line) => {
        const [metadata, path] = line.split('\\t');
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
    publicPath.replace(/\\.json$/, '.html'),
    publicPath.replace(/\\.json$/, '.internal.json'),
    publicPath.replace(/\\.json$/, '.metadata.json'),
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
assert.equal(restApiMetadata.routeTypeCounts['user-generated'], 26);
assert.ok(!Object.prototype.hasOwnProperty.call(restApiMetadata.routeTypeCounts, 'pg' + '-first'));
for (const route of ['/docs/api', '/api/docs', '/api/docs.json']) {
  assert.ok(
    restApiMetadata.routes.some((entry) => entry.path === route && entry.methods.includes('GET')),
    \`rest-api-rs generated metadata is missing GET \${route}\`,
  );
  assert.ok(restApiInternal.paths[route]?.get, \`rest-api-rs internal OpenAPI is missing GET \${route}\`);
  assert.ok(restApiPublic.paths[route]?.get, \`rest-api-rs public OpenAPI is missing GET \${route}\`);
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
    \`\${serviceName} must stay inside generated API contract coverage\`,
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
  assert.ok(publicPath, \`\${service.service} must include a public runtime OpenAPI artifact\`);
  assert.deepEqual(
    service.generated,
    expectedArtifacts(publicPath),
    \`\${service.service} central artifact list is not canonical\`,
  );
  if (unavailableGitlink(repoRoot, publicPath, gitlinks)) {
    skippedGitlinks += 1;
    continue;
  }

  const internalPath = publicPath.replace(/\\.json$/, '.internal.json');
  const metadataPath = publicPath.replace(/\\.json$/, '.metadata.json');
  for (const artifactPath of [publicPath, internalPath, metadataPath]) {
    assert.ok(existsSync(resolve(repoRoot, artifactPath)), \`\${service.service} is missing \${artifactPath}\`);
  }

  const publicOpenapi = readJson(repoRoot, publicPath);
  const internalOpenapi = readJson(repoRoot, internalPath);
  const metadata = readJson(repoRoot, metadataPath);
  assert.equal(publicOpenapi.openapi, '3.1.0', \`\${service.service} must emit OpenAPI 3.1\`);
  assert.equal(publicOpenapi['x-dd-contract-scope'], 'public');
  assert.equal(internalOpenapi['x-dd-contract-scope'], 'internal');
  assert.equal(publicOpenapi['x-dd-service'], service.service);
  assert.equal(metadata.service, service.service);
  for (const route of metadata.standardDocsRoutes) {
    assert.ok(internalOpenapi.paths[route]?.get, \`\${service.service} internal OpenAPI is missing GET \${route}\`);
    assert.ok(publicOpenapi.paths[route]?.get, \`\${service.service} public OpenAPI is missing GET \${route}\`);
  }
  for (const path of Object.keys(publicOpenapi.paths)) {
    assert.ok(!path.startsWith('/internal/'), \`\${service.service} leaked internal route \${path}\`);
  }
  checkedServices += 1;
}
assert.ok(checkedServices > 0, 'expected at least one available service contract to be checked');

console.log(
  [
    generationOutput,
    validationOutput,
    \`route coverage checked \${checkedServices} service(s); skipped \${skippedGitlinks} uninitialized gitlink service(s)\`,
  ]
    .filter(Boolean)
    .join('\\n'),
);
`;
  await write(path, content);
}

async function patchPolicyFiles() {
  const configPath = 'remote/config/api-contracts.json';
  const config = JSON.parse(await read(configPath));
  config.generatedArtifacts = {
    publicOpenapi: 'generated/api-docs.json',
    publicHtml: 'generated/api-docs.html',
    internalOpenapi: 'generated/api-docs.internal.json',
    metadata: 'generated/api-docs.metadata.json',
  };
  config.visibility.runtimeDocsRule =
    'The standard HTTP docs routes serve only the fail-closed public contract. The internal contract is an unserved build artifact for private SDKs and CI.';
  await write(configPath, `${JSON.stringify(config, null, 2)}\n`);

  const docsPath = 'docs/http-api-openapi-sdk-contract.md';
  let docs = await read(docsPath);
  docs = docs.replace(
    '- `GET /api/docs.json` — canonical OpenAPI 3.1 JSON for that running build.',
    '- `GET /api/docs.json` — fail-closed public OpenAPI 3.1 JSON for that running build. The full internal contract is never served by these public routes.',
  );
  docs = docs.replace(
    '| `generated/api-docs.json` | Full service-local OpenAPI 3.1 document served at `/api/docs.json`. |\n| `generated/api-docs.public.json` | Fail-closed public subset used to generate public SDKs. |\n| `generated/api-docs.metadata.json` | Migration/debug metadata about discovered source routes; not a consumer contract. |\n| `generated/api-docs.html` | Human-readable route reference served by the two HTML routes. |',
    '| `generated/api-docs.json` | Fail-closed public OpenAPI 3.1 document served at `/api/docs.json` and used for public SDKs. |\n| `generated/api-docs.html` | Public-only human-readable reference served by the two HTML routes. |\n| `generated/api-docs.internal.json` | Full, unserved contract used only for private SDKs and CI parity checks. |\n| `generated/api-docs.metadata.json` | Migration/debug metadata about discovered source routes; not a consumer contract. |',
  );
  docs = docs.replace(
    'Public packages are generated from `api-docs.public.json`; workspace/private packages are generated\nfrom `api-docs.json`.',
    'Public packages are generated from the runtime-safe `api-docs.json`; workspace/private packages are generated\nfrom the unserved `api-docs.internal.json`.',
  );
  docs = docs.replace(
    '- every indexed available service has full, public, and metadata artifacts;',
    '- every indexed available service has public runtime, public HTML, internal, and metadata artifacts;',
  );
  docs = docs.replace(
    '- the public document is the exact public subset; and',
    '- the runtime document is the exact public subset of the unserved internal document; and',
  );
  await write(docsPath, docs);

  const agentsPath = 'AGENTS.md';
  let agents = await read(agentsPath);
  agents = agents.replace(
    'regenerate/check both the full and fail-closed public OpenAPI artifacts before SDK publication.',
    'regenerate/check the unserved internal contract and the fail-closed public OpenAPI served at `/api/docs.json` before SDK publication.',
  );
  await write(agentsPath, agents);
}

await patchGenerator();
await patchValidator();
await patchParityTest();
await patchPolicyFiles();
await removeLegacyPublicAliases(resolve(repoRoot, 'remote/deployments'));
console.log('made runtime API documentation public-only and retained unserved internal contracts');
