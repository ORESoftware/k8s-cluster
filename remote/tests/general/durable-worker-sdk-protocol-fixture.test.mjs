import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');
const fixture = JSON.parse(read('remote/worker-sdks/fixtures/durable-worker-protocol-v1.json'));

const sdk = {
  typescript: {
    client: read('remote/worker-sdks/typescript/durable-worker/index.mjs'),
    worker: read('remote/worker-sdks/typescript/durable-worker/index.mjs'),
    tests: read('remote/worker-sdks/typescript/durable-worker/test/client.test.mjs'),
    readme: read('remote/worker-sdks/typescript/durable-worker/README.md'),
  },
  python: {
    client: read('remote/worker-sdks/python/durable-worker/src/oresoftware_durable_worker/__init__.py'),
    worker: read('remote/worker-sdks/python/durable-worker/src/oresoftware_durable_worker/__init__.py'),
    tests: read('remote/worker-sdks/python/durable-worker/tests/test_client.py'),
    readme: read('remote/worker-sdks/python/durable-worker/README.md'),
  },
  go: {
    client: read('remote/worker-sdks/go/durable-worker/client.go'),
    worker: read('remote/worker-sdks/go/durable-worker/worker.go'),
    tests: [
      read('remote/worker-sdks/go/durable-worker/client_test.go'),
      read('remote/worker-sdks/go/durable-worker/worker_test.go'),
    ].join('\n'),
    readme: read('remote/worker-sdks/go/durable-worker/README.md'),
  },
  rust: {
    client: read('remote/worker-sdks/rust/durable-worker/src/client.rs'),
    transport: read('remote/worker-sdks/rust/durable-worker/src/transport.rs'),
    worker: read('remote/worker-sdks/rust/durable-worker/src/worker.rs'),
    tests: [
      read('remote/worker-sdks/rust/durable-worker/tests/client.rs'),
      read('remote/worker-sdks/rust/durable-worker/tests/worker.rs'),
    ].join('\n'),
    readme: read('remote/worker-sdks/rust/durable-worker/README.md'),
  },
  dart: {
    client: read('remote/worker-sdks/dart/durable_worker/lib/src/client.dart'),
    worker: read('remote/worker-sdks/dart/durable_worker/lib/src/worker.dart'),
    tests: read('remote/worker-sdks/dart/durable_worker/tool/test.dart'),
    readme: read('remote/worker-sdks/dart/durable_worker/README.md'),
  },
};

function section(source, start, end) {
  const startIndex = source.indexOf(start);
  assert.notEqual(startIndex, -1, `missing section ${start}`);
  const endIndex = source.indexOf(end, startIndex + start.length);
  assert.notEqual(endIndex, -1, `missing section terminator ${end}`);
  return source.slice(startIndex, endIndex);
}

test('shared fixture defines the common at-least-once protocol and five-SDK matrix', () => {
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
  assert.deepEqual(fixture.handAuthoredSdks, ['typescript', 'python', 'go', 'rust', 'dart']);
  assert.deepEqual(fixture.conformanceDimensions, [
    'retry-safety',
    'transport-containment',
    'lease-fencing',
  ]);
  assert.equal(fixture.progressChunkId, '{stepId}:{leaseGeneration}:{sequence}');
  assert.equal(fixture.assignment.leaseGeneration, 3);
  assert.equal(fixture.assignment.fencingToken, 9);
});

// 1/15
test('TypeScript retry-safety matches the shared protocol', () => {
  const submitTask = section(sdk.typescript.client, '  submitTask(', '  submitRun(');
  const submitRun = section(sdk.typescript.client, '  submitRun(', '  getRun(');
  const signal = section(sdk.typescript.client, '  signalRun(', '  pauseRun(');
  const poll = section(sdk.typescript.client, '  pollWorker(', '  startStep(');
  assert.match(submitTask, /idempotent: Boolean\(task\?\.idempotencyKey\)/u);
  assert.match(submitRun, /idempotent: Boolean\(run\?\.idempotencyKey\)/u);
  assert.match(signal, /idempotent: false/u);
  assert.match(poll, /idempotent: false/u);
  assert.match(sdk.typescript.tests, /retries only idempotently bound submissions/u);
  assert.match(sdk.typescript.tests, /does not retry ambiguous polls or signals/u);
});

// 2/15
test('TypeScript transport-containment rejects redirect and credential forwarding hazards', () => {
  assert.match(sdk.typescript.client, /redirect: 'manual'/u);
  assert.match(sdk.typescript.client, /url\.username \|\| url\.password \|\| url\.search \|\| url\.hash/u);
  assert.match(sdk.typescript.client, /authSecret must be a single-line value/u);
  assert.match(sdk.typescript.tests, /rejects credential-bearing base URLs and multiline secrets/u);
});

// 3/15
test('TypeScript lease-fencing aborts handlers and suppresses stale terminal writes', () => {
  assert.match(sdk.typescript.worker, /handlerController\.abort/u);
  assert.match(sdk.typescript.worker, /error instanceof LeaseLostError/u);
  assert.match(sdk.typescript.tests, /aborts the handler and suppresses completion when the lease heartbeat is fenced/u);
  assert.match(sdk.typescript.tests, /assert\.equal\(completeCalls, 0\)/u);
  assert.match(sdk.typescript.tests, /assert\.equal\(failureCalls, 0\)/u);
});

// 4/15
test('Python retry-safety matches the shared protocol', () => {
  const signal = section(sdk.python.client, '    def signal_run(', '    def pause_run(');
  const poll = section(sdk.python.client, '    def poll_worker(', '    def start_step(');
  assert.match(sdk.python.client, /idempotent=bool\(task\.get\("idempotencyKey"\)\)/u);
  assert.match(sdk.python.client, /idempotent=bool\(run\.get\("idempotencyKey"\)\)/u);
  assert.match(signal, /idempotent=False/u);
  assert.match(poll, /idempotent=False/u);
  assert.match(sdk.python.tests, /test_bound_submission_retries_and_unbound_submission_does_not/u);
  assert.match(sdk.python.tests, /test_poll_is_not_retried_after_ambiguous_transport_failure/u);
});

// 5/15
test('Python transport-containment refuses redirects and bounds response bodies', () => {
  assert.match(sdk.python.client, /_NoRedirectHandler/u);
  assert.match(sdk.python.client, /max_response_bytes/u);
  assert.match(sdk.python.tests, /test_redirect_is_never_considered_success/u);
});

// 6/15
test('Python lease-fencing cancels handlers and suppresses stale terminal mutations', () => {
  assert.match(sdk.python.worker, /self\.cancelled\.set\(\)/u);
  assert.match(sdk.python.worker, /cancelled\.set\(\)/u);
  assert.match(sdk.python.tests, /test_fenced_heartbeat_cancels_handler_and_suppresses_terminal_mutations/u);
  assert.match(sdk.python.tests, /summary\.lease_lost/u);
});

// 7/15
test('Go retry-safety matches the shared protocol', () => {
  const signal = section(sdk.go.client, 'func (c *Client) SignalRun(', 'func (c *Client) PauseRun(');
  const poll = section(sdk.go.client, 'func (c *Client) PollWorker(', 'func (c *Client) StartStep(');
  assert.match(signal, /false, false\)/u);
  assert.match(poll, /false, false\)/u);
  assert.match(sdk.go.tests, /TestSubmitRetriesOnlyWithIdempotencyKey/u);
  assert.match(sdk.go.tests, /TestPollIsNotRetriedAfterAmbiguousTransportFailure/u);
  assert.match(sdk.go.tests, /TestSignalIsNotRetriedWithoutProtocolIdentity/u);
});

// 8/15
test('Go transport-containment refuses redirects and enforces response limits', () => {
  assert.match(sdk.go.client, /http\.ErrUseLastResponse/u);
  assert.match(sdk.go.client, /maxResponseBytes/u);
  assert.match(sdk.go.tests, /TestRedirectIsRefusedWithoutForwardingAuthorization/u);
  assert.match(sdk.go.tests, /TestResponseLimitAndInvalidJSON/u);
});

// 9/15
test('Go lease-fencing cancels handlers and suppresses stale terminal mutations', () => {
  assert.match(sdk.go.worker, /context\.WithCancelCause/u);
  assert.match(sdk.go.worker, /cancelTask\(err\)/u);
  assert.match(sdk.go.tests, /TestFencedHeartbeatCancelsHandlerAndSuppressesTerminalMutation/u);
  assert.match(sdk.go.tests, /stale terminal mutation was sent/u);
});

// 10/15
test('Rust retry-safety matches the shared protocol', () => {
  assert.match(sdk.rust.client, /non_empty_string\(&task, "idempotencyKey"\)/u);
  assert.match(sdk.rust.client, /non_empty_string\(&run, "idempotencyKey"\)/u);
  assert.match(sdk.rust.client, /poll_worker[\s\S]*?false,[\s\S]*?false/u);
  assert.match(sdk.rust.client, /signal_run[\s\S]*?false,[\s\S]*?false/u);
  assert.match(sdk.rust.tests, /bound_submission_retries_but_unbound_submission_does_not/u);
  assert.match(sdk.rust.tests, /ambiguous_worker_poll_is_never_retried/u);
});

// 11/15
test('Rust transport-containment disables redirects/proxies and bounds response bodies', () => {
  assert.match(sdk.rust.transport, /redirect\(reqwest::redirect::Policy::none\(\)\)/u);
  assert.match(sdk.rust.transport, /\.no_proxy\(\)/u);
  assert.match(sdk.rust.transport, /max_response_bytes/u);
  assert.match(sdk.rust.tests, /redirect_status_is_not_treated_as_success/u);
});

// 12/15
test('Rust lease-fencing aborts handlers and suppresses stale terminal mutations', () => {
  assert.match(sdk.rust.worker, /handler_handle\.abort\(\)/u);
  assert.match(sdk.rust.worker, /HandlerResolution::LeaseLost/u);
  assert.match(sdk.rust.tests, /fenced_heartbeat_aborts_non_cooperative_handler_and_suppresses_terminal_mutations/u);
  assert.match(sdk.rust.tests, /fenced_progress_output_cancels_handler_and_suppresses_terminal_mutations/u);
});

// 13/15
test('Dart retry-safety matches the shared protocol', () => {
  assert.match(sdk.dart.client, /idempotent: identity != null/u);
  assert.match(sdk.dart.client, /pollWorker[\s\S]*?idempotent: false/u);
  assert.match(sdk.dart.client, /signalRun[\s\S]*?idempotent: false/u);
  assert.match(sdk.dart.tests, /client retry, redirect, body, and lease boundaries/u);
});

// 14/15
test('Dart transport-containment refuses redirects and bounds response bodies', () => {
  assert.match(sdk.dart.client, /request\.followRedirects = false/u);
  assert.match(sdk.dart.client, /_maxResponseBytes/u);
  assert.match(sdk.dart.client, /response_too_large/u);
  assert.match(sdk.dart.tests, /redirectedRequests == 0/u);
});

// 15/15
test('Dart lease-fencing cancels handlers and suppresses stale terminal writes', () => {
  assert.match(sdk.dart.worker, /taskCancellation\.cancel/u);
  assert.match(sdk.dart.worker, /summary\.leaseLost \+= 1/u);
  assert.match(sdk.dart.tests, /heartbeat fencing cancels handlers and suppresses terminal writes/u);
  assert.match(sdk.dart.tests, /output fencing cancels handlers and suppresses terminal writes/u);
  assert.match(sdk.dart.tests, /!api\.operations\.contains\('complete'\)/u);
  assert.match(sdk.dart.tests, /!api\.operations\.contains\('fail'\)/u);
});

test('all five hand-authored SDK docs retain effect-safety obligations', () => {
  for (const language of fixture.handAuthoredSdks) {
    const content = sdk[language].readme;
    assert.match(content, /at least once|at-least-once/iu, language);
    assert.match(content, /idempotency key/iu, language);
    assert.match(content, /fencing(?: token|_token|Token)/iu, language);
  }
});
