#!/usr/bin/env node
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

function findRepoRoot() {
  for (const candidate of [process.cwd(), resolve(__dirname, '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/tools/generate-api-docs.mjs'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const check = spawnSync('node', ['remote/tools/generate-api-docs.mjs', '--check'], {
  cwd: repoRoot,
  encoding: 'utf8',
  timeout: 60_000,
});
assert.equal(
  check.status,
  0,
  `generated API docs are stale.\nSTDOUT:\n${check.stdout}\nSTDERR:\n${check.stderr}`,
);

const restApiDocs = JSON.parse(
  readFileSync(resolve(repoRoot, 'remote/deployments/rest-api-rs/generated/api-docs.json'), 'utf8'),
);
assert.equal(restApiDocs.routeTypeCounts['user-generated'], 23);
assert.ok(!Object.prototype.hasOwnProperty.call(restApiDocs.routeTypeCounts, 'pg' + '-first'));
for (const path of ['/docs/api', '/api/docs', '/api/docs.json']) {
  assert.ok(
    restApiDocs.routes.some((route) => route.path === path),
    `rest-api-rs generated docs are missing the standard docs route ${path}`,
  );
}
assert.ok(
  restApiDocs.routes.every((route) => !route.path.startsWith('/api/db')),
  'rest-api-rs public generated docs must not expose generic /api/db routes.',
);
assert.ok(
  readFileSync(resolve(repoRoot, 'remote/deployments/rest-api-rs/src/main.rs'), 'utf8').includes(
    'router.nest("/internal/db", db_routes::router())',
  ),
  'generic DB inspection routes, if kept, must live under /internal/db and be explicitly gated.',
);

const index = JSON.parse(
  readFileSync(resolve(repoRoot, 'remote/deployments/generated-api-docs-index.json'), 'utf8'),
);
assert.ok(index.services.length >= 15, 'expected generated API docs for HTTP API deployments');
assert.deepEqual(index.centralDocsRoutes, ['/api-docs', '/api-docs.json']);
assert.deepEqual(index.standardDocsRoutes, ['/docs/api', '/api/docs', '/api/docs.json']);
for (const serviceName of ['dart-server', 'fsharp-ws-server']) {
  assert.ok(
    index.services.some((service) => service.service === serviceName),
    `${serviceName} must stay inside generated API docs coverage`,
  );
}
assert.ok(
  readFileSync(resolve(repoRoot, 'remote/deployments/generated-api-docs-index.html'), 'utf8').includes(
    'dd runtime API docs',
  ),
  'central generated API docs HTML index must be committed and servable by web-home-rs.',
);
for (const service of index.services) {
  const docsPath = service.generated.find((path) => path.endsWith('.json'));
  assert.ok(docsPath, `${service.service} must include generated JSON API docs`);
  const docs = JSON.parse(readFileSync(resolve(repoRoot, docsPath), 'utf8'));
  for (const path of docs.standardDocsRoutes) {
    assert.ok(
      docs.routes.some((route) => route.path === path),
      `${service.service} generated docs are missing standard docs route ${path}`,
    );
  }
}
console.log(check.stdout.trim());
