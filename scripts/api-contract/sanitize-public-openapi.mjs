#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
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
    throw new Error(`${label}: expected one match, found ${matches?.length ?? 0}`);
  }
  return source.replace(pattern, replacement);
}

const generatorPath = 'remote/tools/generate-api-docs.mjs';
let generator = await read(generatorPath);
generator = replaceOnce(
  generator,
  /function buildPublicOpenApi\(openapi\) \{[\s\S]*?\n\}\n\nfunction buildPublicDocs/,
  `function buildPublicOpenApi(openapi) {
  const document = structuredClone(openapi);
  for (const [path, pathItem] of Object.entries(document.paths)) {
    for (const method of [...OPENAPI_METHODS].map((value) => value.toLowerCase())) {
      if (pathItem[method]?.['x-dd-visibility'] !== 'public') {
        delete pathItem[method];
      }
    }
    if (Object.keys(pathItem).length === 0) {
      delete document.paths[path];
    }
  }

  const publicEntries = operationEntriesForDocument(document);
  const publicSourceRouteCount = new Set(
    publicEntries.flatMap((entry) => entry.operation['x-dd-source-paths'] ?? []),
  ).size;
  const publicTags = new Set();
  for (const entry of publicEntries) {
    for (const tag of entry.operation.tags ?? []) {
      publicTags.add(tag);
    }
    for (const extension of [
      'x-dd-auth',
      'x-dd-handlers',
      'x-dd-implementation',
      'x-dd-source-files',
      'x-dd-source-path',
      'x-dd-source-paths',
    ]) {
      delete entry.operation[extension];
    }
    entry.operation.security = [];
  }

  document.tags = (document.tags ?? []).filter((tag) => publicTags.has(tag.name));
  document.components = {};
  document.info.title = \`\${document.info.title} (public)\`;
  document.info.description =
    'Fail-closed public subset. Only operations explicitly marked public are included.';
  document['x-dd-contract-scope'] = 'public';
  document['x-dd-route-count'] = publicSourceRouteCount;
  document['x-dd-operation-count'] = publicEntries.length;
  return document;
}

function buildPublicDocs`,
  'replace public OpenAPI sanitizer',
);

generator = replaceOnce(
  generator,
  /function buildPublicDocs\(docs\) \{[\s\S]*?\n\}\n\nfunction buildDocs/,
  `function buildPublicDocs(docs) {
  const routes = docs.routes
    .filter((route) => openApiVisibility(route) === 'public')
    .map((route) => ({
      ...route,
      handlers: [],
      implementation: '',
      notes: '',
      sourceFiles: [],
    }));
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

function buildDocs`,
  'replace public HTML metadata sanitizer',
);
await write(generatorPath, generator);

const validatorPath = 'remote/tools/validate-openapi-contracts.mjs';
let validator = await read(validatorPath);
validator = validator.replace(
  `    assert(
      JSON.stringify(operationSourceKeys(entry)) === JSON.stringify(operationSourceKeys(fullByKey.get(key))),
      \`\${item.service}: public OpenAPI source-path set drifted for \${key}\`,
    );
`,
  '',
);
const visibilityAssertion = `    assert(
      entry.operation['x-dd-visibility'] === 'public',
      \`\${item.service}: internal operation leaked into runtime OpenAPI: \${key}\`,
    );
`;
if (!validator.includes('public runtime OpenAPI leaked debug extension')) {
  validator = validator.replace(
    visibilityAssertion,
    `${visibilityAssertion}    for (const extension of [
      'x-dd-auth',
      'x-dd-handlers',
      'x-dd-implementation',
      'x-dd-source-files',
      'x-dd-source-path',
      'x-dd-source-paths',
    ]) {
      assert(
        !Object.hasOwn(entry.operation, extension),
        \`\${item.service}: public runtime OpenAPI leaked debug extension \${extension} for \${key}\`,
      );
    }
`,
  );
}
if (!validator.includes('public runtime OpenAPI must not publish internal security schemes')) {
  validator = validator.replace(
    `  const publicEntries = operationEntries(publicOpenapi);
`,
    `  const publicEntries = operationEntries(publicOpenapi);
  assert(
    Object.keys(publicOpenapi.components?.securitySchemes ?? {}).length === 0,
    \`\${item.service}: public runtime OpenAPI must not publish internal security schemes\`,
  );
  const publicTagNames = new Set(publicEntries.flatMap((entry) => entry.operation.tags ?? []));
  assert(
    (publicOpenapi.tags ?? []).every((tag) => publicTagNames.has(tag.name)),
    \`\${item.service}: public runtime OpenAPI contains unused or internal tags\`,
  );
`,
  );
}
await write(validatorPath, validator);

console.log('sanitized public OpenAPI and HTML artifacts');
