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

async function readOneJsonLine(child, timeoutMs = 5_000) {
  return new Promise((resolveLine, rejectLine) => {
    let buffer = '';
    let stderr = '';
    const timer = setTimeout(() => {
      rejectLine(new Error(`runtime timed out: ${stderr || buffer}`));
    }, timeoutMs);
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      buffer += chunk;
      const newline = buffer.indexOf('\n');
      if (newline >= 0) {
        clearTimeout(timer);
        resolveLine(JSON.parse(buffer.slice(0, newline)));
      }
    });
    child.once('error', rejectLine);
    child.once('exit', (code) => {
      if (!buffer.includes('\n')) {
        clearTimeout(timer);
        rejectLine(new Error(`runtime exited ${code}: ${stderr || buffer}`));
      }
    });
  });
}

test('Node functions receive immutable release metadata without secrets', async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-node-release-'));
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

  const child = spawn(process.execPath, [runner], {
    env: process.env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const output = readOneJsonLine(child);
  child.stdin.write(`${JSON.stringify({
    slug: 'release-test',
    definition: {
      id: '11111111-1111-4111-8111-111111111111',
      slug: 'release-test',
      runtime: 'nodejs',
      functionBody: `
        return {
          release: context.release,
          immutableRevision: context.capabilities.immutableRevision,
          meta: context.meta,
        };
      `,
      releaseMode: 'alias',
      alias: 'production',
      revisionId: '22222222-2222-4222-8222-222222222222',
      revisionNumber: 7,
      routingVersion: 12,
      definitionDigest: 'a'.repeat(64),
      labels: [],
      metaData: {},
    },
    request: {},
  })}\n`);

  const result = await output;
  child.kill();
  assert.equal(result.ok, true);
  assert.deepEqual(result.result.release, {
    mode: 'alias',
    alias: 'production',
    revisionId: '22222222-2222-4222-8222-222222222222',
    revisionNumber: 7,
    routingVersion: 12,
    definitionDigest: 'a'.repeat(64),
  });
  assert.equal(result.result.immutableRevision, true);
  assert.equal(result.result.meta.releaseMode, 'alias');
  assert.equal(result.result.meta.revisionNumber, 7);
});

test('immutable revisions get distinct dynamically supervised BEAM workers', {
  skip: !commandExists('erlc'),
  timeout: 15_000,
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-beam-release-'));
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
    '-Werror',
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
os:putenv("LAMBDA_NODEJS_HOST_COMMAND", "cat"),
os:putenv("LAMBDA_REVISION_ROUTING_ENABLED", "0"),
os:putenv("SCINTILLA_DRAIN_MARKER", "${join(build, 'not-draining')}"),
{ok, Root} = lambda_runtime_supervisor:start_link(),
unlink(Root),
Base = "{\\"runtime\\":\\"nodejs\\",\\"containerized\\":false,\\"reuseKey\\":\\"session\\",\\"functionBody\\":\\"return null;\\",",
RevisionOne = iolist_to_binary([Base, "\\"releaseMode\\":\\"revision\\",\\"revisionId\\":\\"11111111-1111-4111-8111-111111111111\\",\\"revisionNumber\\":1}"]),
RevisionTwo = iolist_to_binary([Base, "\\"releaseMode\\":\\"revision\\",\\"revisionId\\":\\"22222222-2222-4222-8222-222222222222\\",\\"revisionNumber\\":2}"]),
{ok, _} = lambda_child_runner:invoke_definition(<<"cat">>, <<"release-test">>, RevisionOne, <<"{}">>, 300000, 5000),
{ok, _} = lambda_child_runner:invoke_definition(<<"cat">>, <<"release-test">>, RevisionTwo, <<"{}">>, 300000, 5000),
Counts = lambda_child_runner:worker_counts(),
2 = maps:get(active, Counts),
2 = maps:get(idle, Counts),
{error, Disabled} = lambda_child_runner:invoke_qualified(
    <<"cat">>, <<"release-test">>, <<"production">>, <<"customer-42">>, <<"{}">>, 300000, 5000
),
true = binary:match(Disabled, <<"unavailable">>) =/= nomatch,
Metrics = lambda_child_runner:metrics(),
true = binary:match(Metrics, <<"dd_lambda_runner_release_routed_invocations_total">>) =/= nomatch,
io:format("BEAM_RELEASE_WORKERS_OK~n"),
exit(Root, shutdown),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 12_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /BEAM_RELEASE_WORKERS_OK/);
});

test('HTTP, async pinning, traffic selection, and rollout controls stay wired', async () => {
  const [http, runner, asyncRuntime, workflow, deployment, flags, matrix] =
    await Promise.all([
      readFile(join(root, 'src/gleam_lambda_runner/http_server.gleam'), 'utf8'),
      readFile(join(root, 'src/lambda_child_runner.erl'), 'utf8'),
      readFile(join(root, 'src/lambda_async.erl'), 'utf8'),
      readFile(join(root, 'src/workflow_engine.erl'), 'utf8'),
      readFile(
        join(root, 'k8s/ec2/dd-gleam-lambda-runner.deployment.yaml'),
        'utf8',
      ),
      readFile(join(root, '.cli-flags.toml'), 'utf8'),
      readFile(join(root, 'docs/serverless-capability-matrix.md'), 'utf8'),
    ]);

  assert.match(http, /"invoke", function_id, "aliases", alias/);
  assert.match(http, /"invoke", function_id, "revisions", revision/);
  assert.match(http, /"invoke-stream", function_id, "aliases", alias/);
  assert.match(http, /"async", "invoke", function_id, "aliases", alias/);
  assert.match(http, /x-scintilla-affinity/);
  assert.match(runner, /sum\(\(target\.value::text\)::integer\) over/);
  assert.match(runner, /affinity_bucket/);
  assert.match(runner, /definition_worker_identifier/);
  assert.match(asyncRuntime, /resolve_qualified_reference/);
  assert.match(asyncRuntime, /"functionRef"/);
  assert.match(workflow, /async_pinned_function_ref/);
  assert.match(deployment, /name:\s*LAMBDA_REVISION_ROUTING_ENABLED[\s\S]*value:\s*'0'/);
  assert.match(flags, /\[flags\.lambda_revision_routing_enabled\]/);
  assert.match(matrix, /Aliases and weighted traffic \| \*\*yes\*\*/);
});
