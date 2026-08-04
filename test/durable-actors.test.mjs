import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '..');

function commandExists(command) {
  return spawnSync(command, ['-version'], { stdio: 'ignore' }).error === undefined;
}

async function actorRuntimeFixture() {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-node-actor-'));
  const runner = join(
    build,
    'remote/deployments/scintilla-run-monorepo/apps/gleam-lambda-runner',
    'child-runtimes/js-function-runner.mjs',
  );
  const subjectDefs = join(
    build,
    'remote/libs/nats/subject-defs/generated/javascript/index.mjs',
  );
  await mkdir(dirname(runner), { recursive: true });
  await mkdir(dirname(subjectDefs), { recursive: true });
  await copyFile(join(root, 'child-runtimes/js-function-runner.mjs'), runner);
  await copyFile(
    join(root, 'child-runtimes/lambda-context.mjs'),
    join(dirname(runner), 'lambda-context.mjs'),
  );
  await writeFile(
    subjectDefs,
    'export const containerPoolLanguageRequestsSubject = (name) => `test.${name}`;\n',
  );
  return { build, runner };
}

test('Node actor storage commits state and alarms as one bounded mutation', async () => {
  const { runner } = await actorRuntimeFixture();
  const child = spawn(process.execPath, [runner], {
    env: {
      ...process.env,
      LAMBDA_ACTOR_STATE_MAX_BYTES: '4096',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  const response = new Promise((resolveResponse, rejectResponse) => {
    const timer = setTimeout(
      () => rejectResponse(new Error(`actor runtime timed out: ${stderr}`)),
      5_000,
    );
    let output = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      output += chunk;
      const newline = output.indexOf('\n');
      if (newline >= 0) {
        clearTimeout(timer);
        resolveResponse(JSON.parse(output.slice(0, newline)));
      }
    });
    child.once('error', rejectResponse);
  });

  child.stdin.write(
    `${JSON.stringify({
      mode: 'actor',
      slug: 'counter-actor',
      definition: {
        id: '11111111-1111-1111-1111-111111111111',
        slug: 'counter-actor',
        runtime: 'nodejs',
        containerized: false,
        functionBody: `
          const before = (await context.actor.storage.get("count")) ?? 0;
          await context.actor.storage.put("count", before + request.add);
          await context.actor.storage.put("profile", { id: request.user });
          await context.actor.alarm.set(1_800_000_000_000);
          return {
            before,
            after: await context.actor.storage.get("count"),
            actorId: context.actor.id,
            durable: context.capabilities.durableActor,
          };
        `,
      },
      actor: {
        id: '22222222-2222-2222-2222-222222222222',
        key: 'room:alpha',
        version: 7,
        state: { count: 2 },
        alarmAt: null,
      },
      request: { add: 3, user: 'u-1' },
    })}\n`,
  );

  const result = await response;
  child.kill();
  assert.equal(result.ok, true, stderr);
  assert.deepEqual(result.result, {
    before: 2,
    after: 5,
    actorId: '22222222-2222-2222-2222-222222222222',
    durable: true,
  });
  assert.deepEqual(result.actor.state, {
    count: 5,
    profile: { id: 'u-1' },
  });
  assert.equal(result.actor.alarmAt, '2027-01-15T08:00:00.000Z');
});

test('hot actors are temporary children in a coherent OTP generation', {
  skip: !commandExists('erlc'),
  timeout: 15_000,
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-beam-actor-'));
  execFileSync('erlc', [
    '-o',
    build,
    join(root, 'src/lambda_actor_store.erl'),
    join(root, 'src/lambda_actor.erl'),
    join(root, 'src/lambda_actor_manager.erl'),
    join(root, 'src/lambda_actor_alarm.erl'),
    join(root, 'src/lambda_actor_supervisor.erl'),
  ]);

  const probe = `
process_flag(trap_exit, true),
os:unsetenv("LAMBDA_DATABASE_URL"),
{ok, Root} = lambda_actor_supervisor:start_link(<<"unused">>),
unlink(Root),
true = lambda_actor_supervisor:healthy(),
{ok, Actor} = lambda_actor_supervisor:start_actor(<<"counter-actor">>, <<"room:alpha">>),
true = erlang:is_process_alive(Actor),
Counts = maps:from_list(supervisor:count_children(lambda_actor_worker_supervisor)),
1 = maps:get(active, Counts),
OldManager = whereis(lambda_actor_manager),
OldWorkers = whereis(lambda_actor_worker_supervisor),
ActorMonitor = erlang:monitor(process, Actor),
exit(OldManager, kill),
receive {'DOWN', ActorMonitor, process, Actor, _} -> ok after 2000 -> error(actor_generation_not_drained) end,
timer:sleep(100),
true = lambda_actor_supervisor:healthy(),
true = OldManager =/= whereis(lambda_actor_manager),
true = OldWorkers =/= whereis(lambda_actor_worker_supervisor),
Snapshot = lambda_actor_manager:snapshot(),
io:format("ACTOR_SNAPSHOT:~s~n", [Snapshot]),
exit(Root, shutdown),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 12_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const snapshotLine = result.stdout
    .split('\n')
    .find((line) => line.startsWith('ACTOR_SNAPSHOT:'));
  assert.ok(snapshotLine, result.stderr || result.stdout);
  const snapshot = JSON.parse(snapshotLine.slice('ACTOR_SNAPSHOT:'.length));
  assert.equal(snapshot.ok, true);
  assert.equal(snapshot.dynamicChildren, true);
  assert.equal(snapshot.crossReplicaLease, true);
  assert.equal(snapshot.activeActors, 0);
});

test('durable actor contract is fenced, bounded, observable, and authenticated', async () => {
  const [
    store,
    actor,
    manager,
    supervisor,
    app,
    http,
    deployment,
    flags,
  ] = await Promise.all([
    readFile(join(root, 'src/lambda_actor_store.erl'), 'utf8'),
    readFile(join(root, 'src/lambda_actor.erl'), 'utf8'),
    readFile(join(root, 'src/lambda_actor_manager.erl'), 'utf8'),
    readFile(join(root, 'src/lambda_actor_supervisor.erl'), 'utf8'),
    readFile(join(root, 'src/gleam_lambda_runner.gleam'), 'utf8'),
    readFile(join(root, 'src/gleam_lambda_runner/http_server.gleam'), 'utf8'),
    readFile(
      join(root, 'k8s/ec2/dd-gleam-lambda-runner.deployment.yaml'),
      'utf8',
    ),
    readFile(join(root, '.cli-flags.toml'), 'utf8'),
  ]);

  assert.match(store, /lease_owner = :'owner'/);
  assert.match(store, /state_version = :'version'::bigint/);
  assert.match(store, /match -> "id = :'function_ref'"/);
  assert.match(store, /nullif\(:'alarm_at', ''\)::timestamptz/);
  assert.match(store, /alarm_attempt < 6/);
  assert.match(actor, /claim_with_wait/);
  assert.match(actor, /lambda_actor_store:commit/);
  assert.match(manager, /ACTOR_MAX_QUEUE_DEPTH/);
  assert.match(manager, /dd_lambda_runner_active_actors/);
  assert.match(supervisor, /strategy => one_for_all/);
  assert.match(supervisor, /restart => temporary/);
  assert.match(app, /supervisor\.add\(actors\.supervised\(\)\)/);
  assert.match(http, /Post, \["actors", function_id, actor_key\]/);
  assert.match(http, /require_authenticated_post/);
  assert.match(http, /Delete, \["actors", function_id, actor_key\]/);
  assert.match(deployment, /ACTOR_ALARM_MAX_CONCURRENCY/);
  assert.match(
    deployment,
    /name:\s*ACTOR_ENGINE_ENABLED[\s\S]{0,160}value:\s*'0'/,
  );
  assert.match(flags, /\[flags\.actor_lease_ms\]/);
  assert.match(flags, /\[flags\.lambda_actor_state_max_bytes\]/);
});
