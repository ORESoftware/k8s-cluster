import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');
const root = 'remote/worker-sdks/dart/durable_worker';
const manifest = read(`${root}/pubspec.yaml`);
const lock = read(`${root}/pubspec.lock`);
const client = read(`${root}/lib/src/client.dart`);
const worker = read(`${root}/lib/src/worker.dart`);
const runtimeTests = read(`${root}/tool/test.dart`);
const readme = read(`${root}/README.md`);
const inventory = read('remote/worker-sdks/README.md');
const deliveryDoc = read('docs/durable-worker-dart-sdk.md');
const roadmap = read('docs/durable-worker-roadmap.md');
const workflow = read('.github/workflows/durable-worker-dart-sdk.yml');

test('Dart SDK is dependency-free and locked to the supported language range', () => {
  assert.match(manifest, /name: oresoftware_durable_worker/);
  assert.match(manifest, /sdk: ">=3\.4\.0 <4\.0\.0"/);
  assert.doesNotMatch(manifest, /^dependencies:/m);
  assert.doesNotMatch(manifest, /^dev_dependencies:/m);
  assert.match(lock, /packages: \{\}/);
  assert.match(lock, /dart: ">=3\.4\.0 <4\.0\.0"/);
});

test('client preserves retry, redirect, response, and lease boundaries', () => {
  assert.match(client, /idempotencyKey/);
  assert.match(client, /idempotent: identity != null/);
  assert.match(client, /pollWorker[\s\S]*?idempotent: false/);
  assert.match(client, /signalRun[\s\S]*?idempotent: false/);
  assert.match(client, /request\.followRedirects = false/);
  assert.match(client, /_maxResponseBytes/);
  assert.match(client, /response_too_large/);
  assert.match(client, /durable-worker response body timed out/);
  assert.match(client, /decoded is! Map<Object\?, Object\?>/);
  assert.match(client, /HttpStatus\.notFound \|\| status == HttpStatus\.conflict/);
  assert.match(client, /LeaseLostException/);
});

test('worker owns bounded concurrency, heartbeats, progress, and stale suppression', () => {
  assert.match(worker, /active\.length >= config\.slots/);
  assert.match(worker, /_workerHeartbeatLoop/);
  assert.match(worker, /_stepHeartbeatLoop/);
  assert.match(worker, /final Set<void Function\(Object\)> _listeners/);
  assert.match(worker, /Timer\(duration/);
  assert.match(worker, /timer\?\.cancel\(\)/);
  assert.doesNotMatch(
    worker,
    /Future<void>\.delayed\(Duration\(milliseconds: assignment\.timeoutMs\)\)/,
  );
  assert.match(
    worker,
    /'\$\{assignment\.stepId\}:\$\{_lease\.leaseGeneration\}:\$_sequence'/,
  );
  assert.match(worker, /taskCancellation\.cancel/);
  assert.match(worker, /summary\.leaseLost \+= 1/);
  assert.match(runtimeTests, /step-1:3:1/);
  assert.match(runtimeTests, /step-1:3:2/);
  assert.match(runtimeTests, /bodyTimeoutRequests == 2/);
  assert.match(runtimeTests, /client\.getRun\('non-object'\)/);
  assert.match(
    runtimeTests,
    /heartbeat fencing cancels handlers and suppresses terminal writes/,
  );
  assert.match(
    runtimeTests,
    /output fencing cancels handlers and suppresses terminal writes/,
  );
  assert.match(runtimeTests, /!api\.operations\.contains\('complete'\)/);
  assert.match(runtimeTests, /!api\.operations\.contains\('fail'\)/);
});

test('shared fixture and delivery documentation remain explicit', () => {
  assert.match(runtimeTests, /durable-worker-protocol-v1\.json/);
  assert.match(runtimeTests, /at-least-once/);
  assert.match(readme, /at least once/i);
  assert.match(readme, /fencingToken/);
  assert.match(
    readme,
    /worker polling, signals, and unbound task\/run submissions are sent once/i,
  );
  assert.match(inventory, /`dart\/durable_worker`/);
  assert.match(deliveryDoc, /DEN-2464/);
  assert.match(deliveryDoc, /#1163/);
  assert.match(deliveryDoc, /M3 SDK fleet/);
  assert.match(roadmap, /Dart worker SDK/);
});

test('focused CI is pinned, read-only, multi-version, and deterministic', () => {
  assert.match(workflow, /permissions:\n  contents: read/);
  assert.match(
    workflow,
    /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/,
  );
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /submodules: false/);
  assert.match(
    workflow,
    /dart-lang\/setup-dart@65eb853c7ba17dde3be364c3d2858773e7144260/,
  );
  assert.match(workflow, /- '3\.4\.0'/);
  assert.match(workflow, /- '3\.12\.2'/);
  assert.match(workflow, /dart pub get --enforce-lockfile/);
  assert.match(workflow, /dart format \.[\s\S]*?git diff --exit-code/);
  assert.match(workflow, /dart analyze --fatal-infos --fatal-warnings/);
  assert.match(workflow, /dart run tool\/test\.dart/);
  assert.match(workflow, /seq 1 50/);
  assert.match(
    workflow,
    /tar[\s\S]*?--sort=name[\s\S]*?--mtime='UTC 1970-01-01'[\s\S]*?gzip -n/,
  );
  assert.match(
    workflow,
    /actions\/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02/,
  );
  assert.doesNotMatch(
    workflow,
    /contents: write|pull-requests: write|persist-credentials: true/,
  );
  const trackedDartState = execFileSync(
    'git',
    ['ls-files', `${root}/.dart_tool`],
    { cwd: repoRoot, encoding: 'utf8' },
  ).trim();
  assert.equal(
    trackedDartState,
    '',
    'transient Dart build state must not be committed',
  );
});
