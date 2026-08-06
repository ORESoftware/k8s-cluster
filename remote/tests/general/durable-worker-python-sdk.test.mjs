import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');
const root = 'remote/worker-sdks/python/durable-worker';
const pyproject = read(`${root}/pyproject.toml`);
const source = read(`${root}/src/oresoftware_durable_worker/__init__.py`);
const tests = read(`${root}/tests/test_client.py`);
const readme = read(`${root}/README.md`);
const workflow = read('.github/workflows/durable-worker-python-sdk.yml');
const namespaceReadme = read('remote/worker-sdks/README.md');

test('Python worker SDK is dependency-free and explicitly packaged', () => {
  assert.match(pyproject, /name = "oresoftware-durable-worker"/);
  assert.match(pyproject, /requires-python = ">=3\.11"/);
  assert.match(pyproject, /dependencies = \[\]/);
  assert.match(pyproject, /package-dir = \{"" = "src"\}/);
  assert.match(source, /__all__ = \[/);
  assert.doesNotMatch(source, /\brequests\b|\bhttpx\b|\baiohttp\b/);
});

test('retry boundaries preserve at-least-once safety', () => {
  assert.match(source, /idempotent=bool\(task\.get\("idempotencyKey"\)\)/);
  assert.match(source, /idempotent=bool\(run\.get\("idempotencyKey"\)\)/);
  assert.match(source, /def poll_worker[\s\S]*?idempotent=False/);
  assert.match(source, /class _NoRedirectHandler/);
  assert.match(source, /self\.auth_header: self\.auth_secret/);
  assert.doesNotMatch(source, /auth_secret.*base_url|base_url.*auth_secret/);
  assert.match(tests, /test_poll_is_not_retried_after_ambiguous_transport_failure/);
  assert.match(tests, /test_redirect_is_never_considered_success/);
});

test('worker lifecycle owns heartbeats, fencing, progress, and stale-result suppression', () => {
  assert.match(source, /class TaskContext/);
  assert.match(source, /def fencing_token/);
  assert.match(source, /def _worker_heartbeat_loop/);
  assert.match(source, /def _step_heartbeat_loop/);
  assert.match(source, /cancelled\.set\(\)/);
  assert.match(source, /context\.raise_if_cancelled\(\)/);
  assert.match(source, /client\.complete_step/);
  assert.match(source, /client\.fail_step/);
  assert.match(tests, /test_fenced_heartbeat_cancels_handler_and_suppresses_terminal_mutations/);
  assert.match(tests, /self\.assertNotIn\("complete", operations\)/);
  assert.match(tests, /self\.assertNotIn\("fail", operations\)/);
  assert.match(tests, /test_worker_streams_progress_and_completes_with_same_generation/);
});

test('documentation states the operational safety boundary', () => {
  assert.match(readme, /at-least-once/i);
  assert.match(readme, /fencing_token/);
  assert.match(readme, /redirects are refused/i);
  assert.match(readme, /stale-terminal suppression/i);
  assert.match(namespaceReadme, /Python/);
});

test('focused CI is pinned, read-only, multi-version, and stress-tests fencing', () => {
  assert.match(workflow, /permissions:\n  contents: read/);
  assert.match(workflow, /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/);
  assert.match(workflow, /actions\/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1/);
  for (const version of ['3.11', '3.12', '3.13']) {
    assert.match(workflow, new RegExp(`- '${version.replace('.', '\\.')}'`));
  }
  assert.match(workflow, /seq 1 20/);
  assert.match(workflow, /git diff --exit-code/);
  assert.doesNotMatch(workflow, /contents: write|pull-requests: write|persist-credentials: true/);
});
