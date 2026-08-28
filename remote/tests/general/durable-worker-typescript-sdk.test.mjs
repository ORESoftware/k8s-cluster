import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');

const root = 'remote/worker-sdks/typescript/durable-worker';
const packageJson = JSON.parse(read(`${root}/package.json`));
const source = read(`${root}/index.mjs`);
const declarations = read(`${root}/index.d.ts`);
const sdkTests = read(`${root}/test/client.test.mjs`);
const readme = read(`${root}/README.md`);
const workflow = read('.github/workflows/durable-worker-typescript-sdk.yml');
const namespaceWorkflow = read('.github/workflows/durable-worker-sdk-namespace.yml');

test('the worker SDK is dependency-free native ESM with an explicit package payload', () => {
  assert.equal(packageJson.name, '@oresoftware/durable-worker-sdk');
  assert.equal(packageJson.type, 'module');
  assert.equal(packageJson.sideEffects, false);
  assert.equal(packageJson.private, true);
  assert.equal(packageJson.dependencies, undefined);
  assert.deepEqual(packageJson.files, ['index.mjs', 'index.d.ts', 'README.md']);
  assert.equal(packageJson.exports['.'].import, './index.mjs');
  assert.equal(packageJson.exports['.'].types, './index.d.ts');
  assert.match(packageJson.engines.node, />=22/);
});

test('submission retries are bound to explicit idempotency and credentials are header-only', () => {
  assert.match(source, /idempotent:\s*Boolean\(task\?\.idempotencyKey\)/);
  assert.match(source, /idempotent:\s*Boolean\(run\?\.idempotencyKey\)/);
  assert.match(source, /\[this\.authHeader\]:\s*this\.authSecret/);
  assert.doesNotMatch(source, /console\.(?:log|info|debug).*authSecret/);
  assert.doesNotMatch(source, /authorization.*Bearer/i);
  assert.match(sdkTests, /assert\.equal\(attempts\.get\('unbound'\), 1\)/);
  assert.match(readme, /will not turn\s+an ambiguous network failure into a duplicate run/i);
});

test('worker execution owns heartbeats, progress, bounded admission, drain, and fencing', () => {
  assert.match(source, /#workerHeartbeatLoop/);
  assert.match(source, /heartbeatStep\(assignment\.stepId/);
  assert.match(source, /context = Object\.freeze/);
  assert.match(source, /progress:\s*\(chunk, output = \{\}\)/);
  assert.match(source, /maxAssignments/);
  assert.match(source, /drain:\s*true/);
  assert.match(source, /handlerController\.abort\(leaseLossError\)/);
  assert.match(source, /return 'leaseLost'/);
  assert.match(readme, /downstream write guarded by `context\.fencingToken`/);
});

test('public declarations cover submissions, workers, assignments, handlers, and summaries', () => {
  for (const contract of [
    'SubmitTaskRequest',
    'SubmitRunRequest',
    'StepAssignment',
    'AssignmentContext',
    'TaskHandler',
    'RunWorkerOptions',
    'WorkerRunSummary',
    'DurableWorkerClient',
    'LeaseLostError',
  ]) {
    assert.match(declarations, new RegExp(`(?:interface|type|class) ${contract}\\b`));
  }
  assert.match(declarations, /deadlineMs\?: number/);
  assert.match(declarations, /fencingToken: number/);
  assert.match(declarations, /signal: AbortSignal/);
});

test('runtime tests exercise success, explicit failure, safe retry, and lease loss', () => {
  assert.match(sdkTests, /retries only idempotently bound submissions/);
  assert.match(sdkTests, /automatic heartbeats, progress, completion, and drain/);
  assert.match(sdkTests, /explicit non-retryable failure mutation/);
  assert.match(sdkTests, /lease heartbeat is fenced/);
  assert.match(sdkTests, /instanceof LeaseLostError/);
  assert.match(sdkTests, /completeCalls, 0/);
  assert.match(sdkTests, /failureCalls, 0/);
});

test('SDK CI is pinned, read-only, package-aware, and path-scoped', () => {
  assert.match(workflow, /permissions:\s+contents:\s*read/);
  assert.match(workflow, /actions\/checkout@[0-9a-f]{40}/);
  assert.match(workflow, /actions\/setup-node@[0-9a-f]{40}/);
  assert.match(workflow, /node-version:\s*'22\.23\.1'/);
  assert.match(workflow, /remote\/worker-sdks\/typescript\/durable-worker/);
  assert.match(workflow, /npm run check/);
  assert.match(workflow, /npm test/);
  assert.match(workflow, /npm pack --dry-run --json/);
  assert.match(workflow, /durable-worker-typescript-sdk\.test\.mjs/);
  assert.doesNotMatch(workflow, /contents:\s*write/);
});

test('hand-authored worker SDKs are isolated from generated OpenAPI output', () => {
  assert.match(root, /^remote\/worker-sdks\//);
  assert.match(namespaceWorkflow, /permissions:\s+contents:\s*read/);
  assert.match(namespaceWorkflow, /generate-api-sdks\.mjs --check/);
  assert.match(namespaceWorkflow, /test ! -e "\$old"/);
  assert.doesNotMatch(namespaceWorkflow, /contents:\s*write/);
  assert.doesNotMatch(namespaceWorkflow, /git push/);
});
