import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
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
  assert.match(kustomization, /dd-gleam-lambda-runner\.pdb\.yaml/);
  assert.match(disruptionBudget, /minAvailable:\s*1/);
});
