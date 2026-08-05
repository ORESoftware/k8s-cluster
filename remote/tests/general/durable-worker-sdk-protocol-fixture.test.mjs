import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');
const fixture = JSON.parse(read('remote/worker-sdks/fixtures/durable-worker-protocol-v1.json'));
const typescript = read('remote/worker-sdks/typescript/durable-worker/index.mjs');
const python = read('remote/worker-sdks/python/durable-worker/src/oresoftware_durable_worker/__init__.py');
const goClient = read('remote/worker-sdks/go/durable-worker/client.go');
const goWorker = read('remote/worker-sdks/go/durable-worker/worker.go');

function section(source, start, end) {
  const startIndex = source.indexOf(start);
  assert.notEqual(startIndex, -1, `missing section ${start}`);
  const endIndex = source.indexOf(end, startIndex + start.length);
  assert.notEqual(endIndex, -1, `missing section terminator ${end}`);
  return source.slice(startIndex, endIndex);
}

test('shared fixture defines the common at-least-once protocol identity', () => {
  assert.equal(fixture.version, 1);
  assert.equal(fixture.delivery, 'at-least-once');
  assert.deepEqual(fixture.effectSafety, ['idempotency-key', 'fencing-token']);
  assert.deepEqual(fixture.transientStatuses, [408, 425, 429, 500, 502, 503, 504]);
  assert.deepEqual(fixture.leaseLostStatuses, [404, 409]);
  assert.deepEqual(fixture.neverRetryWithoutIdentity, [
    'submit-task-without-idempotency-key',
    'submit-run-without-idempotency-key',
    'signal-run',
    'worker-poll',
  ]);
  assert.equal(fixture.progressChunkId, '{stepId}:{leaseGeneration}:{sequence}');
  assert.equal(fixture.assignment.leaseGeneration, 3);
  assert.equal(fixture.assignment.fencingToken, 9);
});

test('TypeScript no longer retries ambiguous signals or polls and refuses redirects', () => {
  const signal = section(typescript, '  signalRun(', '  pauseRun(');
  const poll = section(typescript, '  pollWorker(', '  startStep(');
  const loopPoll = section(typescript, "phase: 'poll-ambiguous'", '        if (!polled?.assignment)');
  assert.match(signal, /idempotent: false/u);
  assert.match(poll, /idempotent: false/u);
  assert.match(typescript, /redirect: 'manual'/u);
  assert.doesNotMatch(loopPoll, /continue;/u);
  assert.match(loopPoll, /throw error;/u);
});

test('Python sends ambiguous signals and polls once and cancels fenced handlers', () => {
  const signal = section(python, '    def signal_run(', '    def pause_run(');
  const poll = section(python, '    def poll_worker(', '    def start_step(');
  assert.match(signal, /idempotent=False/u);
  assert.match(poll, /idempotent=False/u);
  assert.match(python, /_NoRedirectHandler/u);
  assert.match(python, /max_response_bytes/u);
  assert.match(python, /self\.cancelled\.set\(\)/u);
});

test('Go sends ambiguous signals and polls once and keeps terminal ambiguity separate', () => {
  const signal = section(goClient, 'func (c *Client) SignalRun(', 'func (c *Client) PauseRun(');
  const poll = section(goClient, 'func (c *Client) PollWorker(', 'func (c *Client) StartStep(');
  assert.match(signal, /false, false\)/u);
  assert.match(poll, /false, false\)/u);
  assert.match(goClient, /http\.ErrUseLastResponse/u);
  assert.match(goClient, /maxResponseBytes/u);
  assert.match(goWorker, /ProtocolErrors/u);
  assert.match(goWorker, /cancelTask\(err\)/u);
});

test('all hand-authored SDK docs retain effect-safety obligations', () => {
  for (const path of [
    'remote/worker-sdks/typescript/durable-worker/README.md',
    'remote/worker-sdks/python/durable-worker/README.md',
    'remote/worker-sdks/go/durable-worker/README.md',
  ]) {
    const content = read(path);
    assert.match(content, /at least once|at-least-once/iu, path);
    assert.match(content, /idempotency key/iu, path);
    assert.match(content, /fencing(?: token|_token|Token)/iu, path);
  }
});
