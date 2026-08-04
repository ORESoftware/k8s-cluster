import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import {
  chmod,
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

test('Node permission launcher adapts to the installed network model', () => {
  const result = spawnSync(
    join(root, 'child-runtimes/node-permission-launcher.sh'),
    ['--version'],
    { encoding: 'utf8' },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^v\d+\./);
});

test('Node runtime streams async iterables as bounded frames', async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-node-stream-'));
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
    env: {
      ...process.env,
      LAMBDA_STREAM_CHUNK_BYTES: '1024',
      LAMBDA_STREAM_MAX_BYTES: '4096',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const frames = [];
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  const finished = new Promise((resolveFinished, rejectFinished) => {
    const timer = setTimeout(() => {
      rejectFinished(new Error(`stream runtime timed out: ${stderr}`));
    }, 5_000);
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
      let newline = stdout.indexOf('\n');
      while (newline >= 0) {
        const line = stdout.slice(0, newline);
        stdout = stdout.slice(newline + 1);
        if (line) {
          const frame = JSON.parse(line);
          frames.push(frame);
          if (frame.event === 'end' || frame.event === 'error') {
            clearTimeout(timer);
            resolveFinished();
          }
        }
        newline = stdout.indexOf('\n');
      }
    });
    child.once('error', rejectFinished);
    child.once('exit', (code) => {
      if (!frames.some((frame) => frame.event === 'end')) {
        clearTimeout(timer);
        rejectFinished(
          new Error(`stream runtime exited ${code}: ${stderr || stdout}`),
        );
      }
    });
  });

  const envelope = {
    mode: 'stream',
    slug: 'stream-test',
    definition: {
      slug: 'stream-test',
      runtime: 'nodejs',
      containerized: false,
      functionBody: `
        return (async function* () {
          yield "alpha";
          yield "x".repeat(2050);
          await new Promise((resolve) => setTimeout(resolve, 20));
          yield new TextEncoder().encode("-beta");
          yield { n: 3 };
        })();
      `,
    },
    request: {},
  };
  child.stdin.write(`${JSON.stringify(envelope)}\n`);
  await finished;
  child.kill();

  assert.equal(frames[0].event, 'start');
  assert.equal(frames.at(-1).event, 'end');
  const chunkFrames = frames.filter((frame) => frame.event === 'chunk');
  assert.ok(chunkFrames.length >= 4, JSON.stringify(frames));
  const output = Buffer.concat(
    chunkFrames.map((frame) => Buffer.from(frame.data, 'base64')),
  ).toString('utf8');
  assert.equal(output, `alpha${'x'.repeat(2050)}-beta{"n":3}\n`);
  assert.equal(frames.at(-1).bytes, Buffer.byteLength(output));
});

test('BEAM stream workers preserve leases and decode framed chunks', {
  skip: !commandExists('erlc'),
  timeout: 15_000,
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-beam-stream-'));
  const configStub = join(build, 'dd_cli_config_client_ffi.erl');
  const streamRuntime = join(build, 'stream-runtime.sh');
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
    streamRuntime,
    `#!/bin/sh
while IFS= read -r line; do
  printf '%s\\n' '{"stream":true,"event":"start","contentType":"text/plain","status":200}'
  printf '%s\\n' '{"stream":true,"event":"chunk","encoding":"base64","data":"YWxwaGE="}'
  printf '%s\\n' '{"stream":true,"event":"chunk","encoding":"base64","data":"b21lZ2E="}'
  printf '%s\\n' '{"stream":true,"event":"end","bytes":10}'
done
`,
  );
  await chmod(streamRuntime, 0o755);

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
os:putenv("LAMBDA_NODEJS_HOST_COMMAND", "${streamRuntime}"),
os:putenv("SCINTILLA_DRAIN_MARKER", "${join(build, 'not-draining')}"),
{ok, Root} = lambda_runtime_supervisor:start_link(),
unlink(Root),
Definition = <<"{\\"runtime\\":\\"nodejs\\",\\"containerized\\":false,\\"maxConcurrency\\":1,\\"functionBody\\":\\"return null;\\"}">>,
Parent = self(),
Emit = fun(Chunk) -> Parent ! {chunk, Chunk} end,
{ok, 10} = lambda_child_runner:invoke_stream_definition(
    <<"ignored">>, <<"streamed">>, Definition, <<"{}">>, 300000, 5000, Emit
),
receive {chunk, <<"alpha">>} -> ok after 1000 -> error(first_chunk_timeout) end,
receive {chunk, <<"omega">>} -> ok after 1000 -> error(second_chunk_timeout) end,
{ok, 10} = lambda_child_runner:invoke_stream_definition(
    <<"ignored">>, <<"streamed">>, Definition, <<"{}">>, 300000, 5000, Emit
),
Counts = lambda_child_runner:worker_counts(),
0 = maps:get(busy, Counts),
1 = maps:get(idle, Counts),
Metrics = lambda_child_runner:metrics(),
true = binary:match(Metrics, <<"dd_lambda_runner_stream_chunks_total">>) =/= nomatch,
true = binary:match(Metrics, <<"dd_lambda_runner_stream_bytes_total">>) =/= nomatch,
io:format("BEAM_STREAM_OK~n"),
exit(Root, shutdown),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 12_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /BEAM_STREAM_OK/);
});

test('HTTP streaming acknowledges socket writes before the next child chunk', async () => {
  const [
    http,
    bridge,
    runner,
    deployment,
    cliFlags,
    launcher,
    nodeImage,
  ] = await Promise.all([
    readFile(join(root, 'src/gleam_lambda_runner/http_server.gleam'), 'utf8'),
    readFile(join(root, 'src/gleam_lambda_runner/child_process.gleam'), 'utf8'),
    readFile(join(root, 'src/lambda_child_runner.erl'), 'utf8'),
    readFile(
      join(root, 'k8s/ec2/dd-gleam-lambda-runner.deployment.yaml'),
      'utf8',
    ),
    readFile(join(root, '.cli-flags.toml'), 'utf8'),
    readFile(
      join(root, 'child-runtimes/node-permission-launcher.sh'),
      'utf8',
    ),
    readFile(join(root, 'runtime-images/nodejs.Dockerfile'), 'utf8'),
  ]);

  assert.match(http, /Post, \["invoke-stream", function_id\]/);
  assert.match(http, /mist\.chunked/);
  assert.match(http, /mist\.send_chunk\(connection, data\)/);
  assert.match(http, /process\.send\(acknowledge, Nil\)/);
  assert.match(bridge, /process\.call_forever/);
  assert.match(runner, /Emit\(Chunk\)/);
  assert.match(runner, /LAMBDA_STREAM_MAX_BYTES/);
  assert.match(deployment, /LAMBDA_STREAM_CHUNK_BYTES/);
  assert.match(cliFlags, /\[flags\.lambda_stream_max_bytes\]/);
  assert.match(cliFlags, /\[flags\.lambda_stream_chunk_bytes\]/);
  assert.match(launcher, /node --help/);
  assert.match(launcher, /--allow-net/);
  assert.match(nodeImage, /node-permission-launcher/);
  assert.match(nodeImage, /--allow-fs-read=\/opt\/scintilla\/node_modules/);
});
