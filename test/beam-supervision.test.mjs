import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { chmod, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '..');

function commandExists(command) {
  return spawnSync(command, ['-version'], { stdio: 'ignore' }).error === undefined;
}

test('warm runtime workers are real dynamic OTP children', {
  skip: !commandExists('erlc'),
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-beam-supervision-'));
  const configStub = join(build, 'dd_cli_config_client_ffi.erl');
  await writeFile(
    configStub,
    `-module(dd_cli_config_client_ffi).
-export([getenv/2]).
getenv(Name, Default) when is_binary(Name) ->
    getenv(binary_to_list(Name), Default);
getenv(Name, Default) when is_list(Name) ->
    case os:getenv(Name) of
        false -> Default;
        Value -> unicode:characters_to_binary(Value)
    end.
`,
  );

  execFileSync('erlc', [
    '-o',
    build,
    configStub,
    join(root, 'src/lambda_child_runner.erl'),
    join(root, 'src/lambda_runtime_supervisor.erl'),
  ]);

  const probe = `
process_flag(trap_exit, true),
os:putenv("LAMBDA_PREWARM_RUNTIMES", "none"),
os:putenv("LAMBDA_PREWARM_CONTAINER_RUNTIMES", ""),
os:putenv("SCINTILLA_DRAIN_MARKER", "${join(build, 'not-draining')}"),
{ok, Root} = lambda_runtime_supervisor:start_link(),
unlink(Root),
true = lambda_runtime_supervisor:healthy(),
{ok, Worker} = lambda_runtime_supervisor:start_worker(<<"cat">>),
true = erlang:is_process_alive(Worker),
OldManager = whereis(lambda_child_runner_manager),
OldWorkerSupervisor = whereis(lambda_runtime_worker_supervisor),
WorkerMonitor = erlang:monitor(process, Worker),
exit(OldManager, kill),
receive {'DOWN', WorkerMonitor, process, Worker, _} -> ok after 2000 -> error(worker_generation_not_drained) end,
timer:sleep(100),
true = lambda_runtime_supervisor:healthy(),
true = OldManager =/= whereis(lambda_child_runner_manager),
true = OldWorkerSupervisor =/= whereis(lambda_runtime_worker_supervisor),
{ok, ReplacementWorker} = lambda_runtime_supervisor:start_worker(<<"cat">>),
Snapshot = lambda_runtime_supervisor:snapshot(),
true = is_binary(Snapshot),
ReplacementWorker ! stop,
ReplacementMonitor = erlang:monitor(process, ReplacementWorker),
receive {'DOWN', ReplacementMonitor, process, ReplacementWorker, normal} -> ok after 2000 -> error(worker_stop_timeout) end,
io:format("SNAPSHOT:~s~n", [Snapshot]),
exit(Root, shutdown),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const encodedSnapshot = result.stdout
    .split('\n')
    .find((line) => line.startsWith('SNAPSHOT:'))
    ?.slice('SNAPSHOT:'.length);
  assert.ok(encodedSnapshot, result.stderr || result.stdout);
  const snapshot = JSON.parse(encodedSnapshot);
  assert.equal(snapshot.ok, true);
  assert.equal(snapshot.supervisionStrategy, 'one_for_all');
  assert.equal(snapshot.dynamicChildren, true);
  assert.equal(snapshot.draining, false);
  assert.equal(snapshot.activeWorkers, 1);
});

test('Kubernetes rollout drains without taking the runner fleet to zero', async () => {
  const deployment = await readFile(
    join(root, 'k8s/ec2/dd-gleam-lambda-runner.deployment.yaml'),
    'utf8',
  );
  const kustomization = await readFile(
    join(root, 'k8s/ec2/kustomization.yaml'),
    'utf8',
  );
  const disruptionBudget = await readFile(
    join(root, 'k8s/ec2/dd-gleam-lambda-runner.pdb.yaml'),
    'utf8',
  );

  assert.match(deployment, /replicas:\s*2/);
  assert.match(deployment, /maxUnavailable:\s*0/);
  assert.match(deployment, /maxSurge:\s*1/);
  assert.match(deployment, /path:\s*\/readyz/);
  assert.match(deployment, /touch \/tmp\/scintilla-draining/);
  assert.match(deployment, /terminationGracePeriodSeconds:\s*330/);
  assert.match(deployment, /name:\s*LAMBDA_DEFAULT_MAX_CONCURRENCY/);
  assert.match(kustomization, /dd-gleam-lambda-runner\.pdb\.yaml/);
  assert.match(disruptionBudget, /minAvailable:\s*1/);
});

test('bounded BEAM worker pools reject excess concurrency without queueing', {
  skip: !commandExists('erlc'),
  timeout: 15_000,
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-beam-concurrency-'));
  const configStub = join(build, 'dd_cli_config_client_ffi.erl');
  const slowRuntime = join(build, 'slow-runtime.sh');
  await writeFile(
    configStub,
    `-module(dd_cli_config_client_ffi).
-export([getenv/2]).
getenv(Name, Default) when is_binary(Name) ->
    getenv(binary_to_list(Name), Default);
getenv(Name, Default) when is_list(Name) ->
    case os:getenv(Name) of
        false -> Default;
        Value -> unicode:characters_to_binary(Value)
    end.
`,
  );
  await writeFile(
    slowRuntime,
    `#!/bin/sh
while IFS= read -r line; do
  sleep 1
  printf '%s\\n' "$line"
done
`,
  );
  await chmod(slowRuntime, 0o755);

  execFileSync('erlc', [
    '-o',
    build,
    configStub,
    join(root, 'src/lambda_child_runner.erl'),
    join(root, 'src/lambda_runtime_supervisor.erl'),
  ]);

  const probe = `
process_flag(trap_exit, true),
os:putenv("LAMBDA_PREWARM_RUNTIMES", "none"),
os:putenv("LAMBDA_PREWARM_CONTAINER_RUNTIMES", ""),
os:putenv("LAMBDA_ALLOW_HOST_RUNTIMES", "nodejs"),
os:putenv("LAMBDA_NODEJS_HOST_COMMAND", "${slowRuntime}"),
os:putenv("SCINTILLA_DRAIN_MARKER", "${join(build, 'not-draining')}"),
{ok, Root} = lambda_runtime_supervisor:start_link(),
unlink(Root),
Definition = <<"{\\"runtime\\":\\"nodejs\\",\\"containerized\\":false,\\"maxConcurrency\\":1,\\"functionBody\\":\\"return request;\\"}">>,
Parent = self(),
spawn(fun() ->
    Parent ! {first, lambda_child_runner:invoke_definition(
        <<"ignored">>, <<"limited">>, Definition, <<"{}">>, 300000, 5000
    )}
end),
timer:sleep(100),
StartedAt = erlang:monotonic_time(millisecond),
Second = lambda_child_runner:invoke_definition(
    <<"ignored">>, <<"limited">>, Definition, <<"{}">>, 300000, 5000
),
Elapsed = erlang:monotonic_time(millisecond) - StartedAt,
{error, ConcurrencyError} = Second,
true = binary:match(ConcurrencyError, <<"concurrency limit reached">>) =/= nomatch,
true = Elapsed < 500,
receive {first, {ok, _}} -> ok after 5000 -> error(first_invocation_timeout) end,
{ok, _} = lambda_child_runner:invoke_definition(
    <<"ignored">>, <<"limited">>, Definition, <<"{}">>, 300000, 5000
),
Counts = lambda_child_runner:worker_counts(),
0 = maps:get(busy, Counts),
1 = maps:get(idle, Counts),
Metrics = lambda_child_runner:metrics(),
true = binary:match(Metrics, <<"dd_lambda_runner_concurrency_rejections_total">>) =/= nomatch,
true = binary:match(Metrics, <<"dd_lambda_runner_busy_workers">>) =/= nomatch,
io:format("BACKPRESSURE:~p:~p~n", [Elapsed, ConcurrencyError]),
exit(Root, shutdown),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 12_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const backpressure = result.stdout
    .split('\n')
    .find((line) => line.startsWith('BACKPRESSURE:'));
  assert.ok(backpressure, result.stderr || result.stdout);
});
