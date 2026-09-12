import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');
const root = 'remote/worker-sdks/rust/durable-worker';
const manifest = read(`${root}/Cargo.toml`);
const client = read(`${root}/src/client.rs`);
const transport = read(`${root}/src/transport.rs`);
const worker = read(`${root}/src/worker.rs`);
const tests = read(`${root}/tests/worker.rs`);
const fixtureTest = read(`${root}/tests/protocol_fixture.rs`);
const readme = read(`${root}/README.md`);
const inventory = read('remote/worker-sdks/README.md');
const deliveryDoc = read('docs/durable-worker-rust-sdk.md');
const workflow = read('.github/workflows/durable-worker-rust-sdk.yml');

test('Rust SDK has a pinned MSRV and audited runtime dependencies', () => {
  assert.match(manifest, /name = "oresoftware-durable-worker"/);
  assert.match(manifest, /rust-version = "1\.85"/);
  assert.match(manifest, /reqwest = \{ version = "0\.12", default-features = false, features = \["rustls-tls"\] \}/);
  assert.match(manifest, /serde = \{ version = "1", features = \["derive"\] \}/);
  assert.match(manifest, /tokio = \{ version = "1"/);
  assert.doesNotMatch(manifest, /openssl|native-tls/);
});

test('client retry boundaries preserve at-least-once safety', () => {
  assert.match(client, /non_empty_string\(&task, "idempotencyKey"\)/);
  assert.match(client, /non_empty_string\(&run, "idempotencyKey"\)/);
  assert.match(client, /poll_worker[\s\S]*?false,[\s\S]*?false/);
  assert.match(client, /signal_run[\s\S]*?false,[\s\S]*?false/);
  assert.match(client, /matches!\(status, 404 \| 409\)/);
  assert.match(transport, /\.no_proxy\(\)/);
  assert.match(transport, /redirect\(reqwest::redirect::Policy::none\(\)\)/);
  assert.match(transport, /max_response_bytes/);
});

test('worker owns bounded heartbeats, progress identity, cancellation, and stale suppression', () => {
  assert.match(worker, /tasks\.len\(\) >= self\.config\.slots/);
  assert.match(worker, /worker_heartbeat_loop/);
  assert.match(worker, /step_heartbeat_loop/);
  assert.match(worker, /"\{\}:\{\}:\{\}"/);
  assert.match(worker, /if cancellation\.is_cancelled\(\) \|\| matches!\(&resolution, HandlerResolution::LeaseLost\)/);
  assert.match(worker, /tokio::select!/);
  assert.match(tests, /fenced_heartbeat_aborts_non_cooperative_handler_and_suppresses_terminal_mutations/);
  assert.match(tests, /fenced_progress_output_cancels_handler_and_suppresses_terminal_mutations/);
  assert.match(worker, /pub protocol_errors: usize/);
  assert.match(worker, /handler_handle\.abort\(\)/);
  assert.match(worker, /assignment\.timeout_ms/);
  assert.match(worker, /heartbeat_request_timeout/);
  assert.match(tests, /assignment_timeout_aborts_handler_and_reports_retryable_failure/);
  assert.match(tests, /blocked_step_heartbeat_aborts_non_cooperative_handler_within_local_budget/);
  assert.match(tests, /handler_panic_is_isolated_and_reported_as_a_terminal_failure/);
  assert.match(tests, /ambiguous_terminal_mutation_is_counted_separately_from_acknowledged_failure/);
  assert.match(tests, /shutdown_cancels_an_in_flight_long_poll/);
  assert.match(tests, /rejects_worker_heartbeat_cadence_that_can_expire_the_ttl/);
  assert.match(tests, /step-1:3:1/);
  assert.match(tests, /step-1:3:2/);
  assert.match(tests, /!operations\.contains\(&"complete"\.to_owned\(\)\)/);
  assert.match(tests, /!operations\.contains\(&"fail"\.to_owned\(\)\)/);
});

test('shared fixture and docs retain the protocol safety contract', () => {
  assert.match(fixtureTest, /durable-worker-protocol-v1\.json/);
  assert.match(fixtureTest, /at-least-once/);
  assert.match(readme, /at least once/i);
  assert.match(readme, /fencing_token/);
  assert.match(readme, /worker polling and signals are sent once/i);
  assert.match(inventory, /`rust\/durable-worker`/);
  assert.match(deliveryDoc, /DEN-2392/);
  assert.match(deliveryDoc, /#1021/);
  assert.match(deliveryDoc, /M3 SDK fleet/);
  assert.match(deliveryDoc, /stale lease generation/);
});

test('focused CI is locked, read-only, pinned, multi-version, and deterministic', () => {
  assert.match(workflow, /permissions:\n  contents: read/);
  assert.match(workflow, /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/);
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /submodules: false/);
  assert.match(workflow, /dtolnay\/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30/);
  assert.match(workflow, /- '1\.85\.0'/);
  assert.match(workflow, /- stable/);
  assert.match(workflow, /cargo metadata --locked --no-deps --format-version 1/);
  assert.match(workflow, /cargo fmt --all -- --check/);
  assert.match(workflow, /cargo clippy --locked --all-targets --all-features -- -D warnings/);
  assert.match(workflow, /cargo test --locked --all-targets --all-features -- --nocapture/);
  assert.match(workflow, /cargo tree --locked --duplicates/);
  assert.match(workflow, /seq 1 50/);
  assert.match(workflow, /tar[\s\S]*?--sort=name[\s\S]*?--mtime='UTC 1970-01-01'[\s\S]*?gzip -n/);
  assert.match(workflow, /actions\/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02/);
  assert.doesNotMatch(workflow, /contents: write|pull-requests: write|persist-credentials: true/);
  assert.equal(
    existsSync(resolve(repoRoot, '.github/workflows/durable-worker-rust-lock-bootstrap.yml')),
    false,
    'the temporary write-capable Cargo.lock bootstrap workflow must stay removed',
  );
});
