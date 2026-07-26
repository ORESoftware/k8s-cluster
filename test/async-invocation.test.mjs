import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '..');

function commandExists(command) {
  return spawnSync(command, ['-version'], { stdio: 'ignore' }).error === undefined;
}

test('async invocation policy is bounded and passed separately from user payload', {
  skip: !commandExists('erlc'),
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-async-policy-'));
  execFileSync('erlc', [
    '-o',
    build,
    join(root, 'src/lambda_async.erl'),
    join(root, 'src/workflow_engine.erl'),
  ]);

  const probe = `
Body = <<"{\\"payload\\":{\\"hello\\":\\"beam\\"},\\"idempotencyKey\\":\\"idem-1\\",\\"retry\\":{\\"maxAttempts\\":7,\\"backoffMs\\":250,\\"backoffFactor\\":2.5,\\"maxBackoffMs\\":5000},\\"maxEventAgeMs\\":60000,\\"timeoutMs\\":45000}">>,
{ok, InputJson, <<"idem-1">>} = lambda_async:validate_request(Body),
Input = json:decode(iolist_to_binary(InputJson)),
#{<<"hello">> := <<"beam">>} = maps:get(<<"payload">>, Input),
Options = maps:get(<<"options">>, Input),
45000 = maps:get(<<"timeoutMs">>, Options),
60000 = maps:get(<<"maxEventAgeMs">>, Options),
Step = #{<<"asyncPolicy">> => true, <<"payloadMode">> => <<"asyncPayload">>},
Run = #{<<"input">> => Input, <<"createdAtMs">> => 1000, <<"nowMs">> => 61001},
Retry = workflow_engine:retry_config(Run, Step),
7 = workflow_engine:max_attempts(Retry),
250 = workflow_engine:backoff_ms(Retry, 0),
{Payload, EncodedPayload} = workflow_engine:activity_payload(<<"run-1">>, Run, Step, 0),
#{<<"hello">> := <<"beam">>} = Payload,
Payload = json:decode(EncodedPayload),
true = workflow_engine:async_event_expired(Run, Step),
LongKey = binary:copy(<<"x">>, 201),
LongBody = json:encode(#{<<"payload">> => null, <<"idempotencyKey">> => LongKey}),
{error, _} = lambda_async:validate_request(LongBody),
{error, _} = lambda_async:validate_request(<<"{\\"retry\\":{\\"maxAttempts\\":0}}">>),
io:format("ASYNC_POLICY_OK~n"),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /ASYNC_POLICY_OK/);
});

test('async persistence uses atomic leasing, idempotency, and managed definitions', async () => {
  const store = await readFile(join(root, 'src/workflow_store.erl'), 'utf8');
  const engine = await readFile(join(root, 'src/workflow_engine.erl'), 'utf8');
  const app = await readFile(join(root, 'src/gleam_lambda_runner.gleam'), 'utf8');
  const http = await readFile(
    join(root, 'src/gleam_lambda_runner/http_server.gleam'),
    'utf8',
  );
  const deployment = await readFile(
    join(root, 'k8s/ec2/dd-gleam-lambda-runner.deployment.yaml'),
    'utf8',
  );

  assert.match(store, /FOR UPDATE SKIP LOCKED/i);
  assert.match(store, /on conflict \(definition_id, idempotency_key\)/i);
  assert.match(store, /meta_data->>'kind' = 'lambdaAsync'/);
  assert.match(
    store,
    /'run', \(select ", run_json_object\("workflow_runs"\)/,
  );
  assert.doesNotMatch(
    store,
    /cancel_run[\s\S]*?\{"lease_until = null"[\s\S]*?deliver_signal/,
  );
  assert.match(engine, /NATS_ASYNC_DLQ_SUBJECT/);
  assert.match(engine, /lambda_async_dlq_published_total/);
  assert.match(app, /supervisor\.add\(workflow\.supervised\(\)\)/);
  assert.match(http, /\["async", "invoke", function_id\]/);
  assert.match(http, /\["async", "invocations", run_id, "cancel"\]/);
  assert.match(deployment, /name:\s*NATS_ASYNC_DLQ_SUBJECT/);
});
