import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const sdk = resolve(root, 'remote/worker-sdks/go/durable-worker');
const read = (path) => readFileSync(resolve(root, path), 'utf8');
const required = [
  'remote/worker-sdks/go/durable-worker/go.mod',
  'remote/worker-sdks/go/durable-worker/client.go',
  'remote/worker-sdks/go/durable-worker/worker.go',
  'remote/worker-sdks/go/durable-worker/client_test.go',
  'remote/worker-sdks/go/durable-worker/worker_test.go',
  'remote/worker-sdks/go/durable-worker/protocol_fixture_test.go',
  'remote/worker-sdks/go/durable-worker/README.md',
  'remote/worker-sdks/go/durable-worker/examples/basic-worker/main.go',
  'remote/worker-sdks/fixtures/durable-worker-protocol-v1.json',
  'remote/tests/general/durable-worker-sdk-protocol-fixture.test.mjs',
  '.github/workflows/durable-worker-go-sdk.yml',
];

const credentialPattern = /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/u;

function filesRecursively(path) {
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const candidate = resolve(path, entry.name);
    return entry.isDirectory() ? filesRecursively(candidate) : [candidate];
  });
}

test('Go worker SDK has the complete dependency-free source surface', () => {
  for (const path of required) assert.equal(existsSync(resolve(root, path)), true, path);
  const module = read('remote/worker-sdks/go/durable-worker/go.mod');
  assert.match(module, /^module github\.com\/oresoftware\/k8s-cluster\/remote\/worker-sdks\/go\/durable-worker$/mu);
  assert.match(module, /^go 1\.23$/mu);
  assert.doesNotMatch(module, /^require\s/mu);
  assert.equal(existsSync(resolve(sdk, 'go.sum')), false, 'stdlib-only SDK must not need go.sum');
  assert.equal(existsSync(resolve(sdk, 'vendor')), false, 'vendored dependencies are not allowed');
});

test('client source encodes safe retry, redirect, response, and lease boundaries', () => {
  const source = read('remote/worker-sdks/go/durable-worker/client.go');
  for (const token of [
    'DisableRetries',
    'CheckRedirect',
    'http.ErrUseLastResponse',
    'maxResponseBytes',
    'response_too_large',
    'LeaseLostError',
    'PollWorker',
    'SignalRun',
    'idempotencyKey',
  ]) assert.match(source, new RegExp(token));
  assert.match(source, /func \(c \*Client\) PollWorker[\s\S]*c\.request\([^;]*false, false\)/u);
  assert.match(source, /func \(c \*Client\) SignalRun[\s\S]*c\.request\([^;]*false, false\)/u);
});

test('worker source owns admission, heartbeats, fencing cancellation, and ambiguous terminals', () => {
  const source = read('remote/worker-sdks/go/durable-worker/worker.go');
  for (const token of [
    'RunWorker',
    'MaxAssignments',
    'WorkerHeartbeat',
    'StepHeartbeat',
    'context.WithCancelCause',
    'lease_heartbeat_uncertain',
    'ProtocolErrors',
    'handler_not_found',
    'handler_panic',
    'FencingToken',
  ]) assert.match(source, new RegExp(token));
  assert.match(source, /semaphore := make\(chan struct\{\}, config\.Slots\)/u);
  assert.match(source, /cancelTask\(err\)/u);
});

test('README and example state the at-least-once effect contract', () => {
  const readme = read('remote/worker-sdks/go/durable-worker/README.md');
  const example = read('remote/worker-sdks/go/durable-worker/examples/basic-worker/main.go');
  assert.match(readme, /at least once/i);
  assert.match(readme, /idempotency key/i);
  assert.match(readme, /fencing(?: token|_token|Token)/i);
  assert.match(readme, /ambiguous outcome stops the loop/i);
  assert.match(example, /FencingToken\(\)/u);
  assert.match(example, /signal\.NotifyContext/u);
});

test('permanent workflow is pinned, read-only, race-tested, and publishes a source artifact only after push', () => {
  const workflow = read('.github/workflows/durable-worker-go-sdk.yml');
  assert.match(workflow, /permissions:\n  contents: read/u);
  assert.match(workflow, /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/u);
  assert.match(workflow, /actions\/setup-go@924ae3a1cded613372ab5595356fb5720e22ba16/u);
  assert.match(workflow, /actions\/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02/u);
  assert.match(workflow, /persist-credentials: false/u);
  assert.match(workflow, /go test \.\/\.\.\. -race -count=1/u);
  assert.match(workflow, /go vet \.\/\.\.\./u);
  assert.match(workflow, /github\.event_name == 'push'/u);
  assert.doesNotMatch(workflow, /contents:\s*write|packages:\s*write|persist-credentials:\s*true/u);
});

test('SDK source contains no credential-shaped material or binary payload', () => {
  for (const path of filesRecursively(sdk)) {
    assert.equal(statSync(path).size < 250_000, true, `${path} is unexpectedly large`);
    const content = readFileSync(path, 'utf8');
    assert.doesNotMatch(content, credentialPattern, path);
  }
});
