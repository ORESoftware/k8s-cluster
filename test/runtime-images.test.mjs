import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import test from 'node:test';

const root = new URL('../', import.meta.url);
const read = (path) => readFile(new URL(path, root), 'utf8');

test('requested compiled runtimes have production and dynamic build stages', async () => {
  const matrix = JSON.parse(await read('runtime-images/matrix.json'));
  assert.equal(matrix.contractVersion, 'scintilla.run/runtime.v1');
  assert.equal(matrix.protocol, 'line-delimited-json-stdio');
  assert.deepEqual(matrix.runtimes.map(({ name }) => name), [
    'nodejs', 'python3', 'ruby', 'bash', 'golang', 'dart',
    'java', 'erlang', 'elixir', 'gleam', 'rust', 'browser',
  ]);
  const requested = new Set(['nodejs', 'golang', 'java', 'erlang', 'elixir', 'gleam', 'rust']);
  for (const runtime of matrix.runtimes.filter(({ name }) => requested.has(name))) {
    const dockerfile = await read(runtime.dockerfile);
    assert.match(dockerfile, /\sAS toolchain\b/i, `${runtime.name}: toolchain stage`);
    assert.match(dockerfile, /\sAS runtime\b/i, `${runtime.name}: runtime stage`);
    assert.match(dockerfile, /\sAS dynamic\b/i, `${runtime.name}: dynamic stage`);
    assert.match(dockerfile, /USER 10001:10001/, `${runtime.name}: non-root user`);
    assert.match(dockerfile, /ENTRYPOINT \["\/usr\/local\/bin\/scintilla-entrypoint"\]/);
    assert.doesNotMatch(dockerfile, /:(?:latest|edge)(?:\s|@|$)/, `${runtime.name}: mutable base`);
  }
});

test('every managed runtime uses the common overridable entrypoint', async () => {
  for (const runtime of [
    'nodejs', 'python3', 'ruby', 'bash', 'golang', 'dart',
    'erlang', 'elixir', 'java', 'gleam', 'rust', 'browser',
  ]) {
    const dockerfile = await read(`runtime-images/${runtime}.Dockerfile`);
    assert.match(dockerfile, /scintilla-entrypoint/);
    assert.match(dockerfile, /\bCMD \[/);
  }
});

test('entrypoint executes a configured override without losing its argument boundaries', async () => {
  const script = new URL('runtime-images/scintilla-entrypoint.sh', root);
  const child = spawn('/bin/sh', [script.pathname, 'unused'], {
    env: {
      PATH: process.env.PATH,
      SCINTILLA_RUNTIME_COMMAND: `printf '%s\\n' "hello from override"`,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk; });
  child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
  const status = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });
  assert.equal(status, 0, stderr);
  assert.equal(stdout, 'hello from override\n');
});

async function executable(name) {
  try {
    await access(`/usr/local/bin/${name}`);
    return `/usr/local/bin/${name}`;
  } catch {
    return name;
  }
}

async function invokePolyglot(runtime, functionBody, request) {
  const node = await executable('node');
  const child = spawn(node, ['child-runtimes/polyglot-function-runner.mjs'], {
    cwd: new URL('.', root),
    env: { ...process.env, LAMBDA_TARGET_RUNTIME: runtime },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
  const line = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
      const end = stdout.indexOf('\n');
      if (end >= 0) resolve(stdout.slice(0, end));
    });
  });
  child.stdin.end(`${JSON.stringify({
    definition: {
      id: '00000000-0000-0000-0000-000000000001',
      slug: `${runtime}-e2e`,
      runtime,
      functionBody,
      maxRunMs: 30_000,
      status: 'active',
    },
    invocationId: 'runtime-test',
    request,
  })}\n`);
  const result = JSON.parse(await line);
  child.kill('SIGTERM');
  assert.equal(result.ok, true, stderr || result.error);
  return result.result;
}

test('Rust source compiles and runs through the real stdio adapter', async () => {
  const result = await invokePolyglot(
    'rust',
    `pub fn handle(request_json: &str, _context_json: &str) -> String {
  format!("{{\\"runtime\\":\\"rust\\",\\"request\\":{request_json}}}")
}`,
    { value: 42 },
  );
  assert.deepEqual(result, { runtime: 'rust', request: { value: 42 } });
});

test('Gleam source compiles and runs through the real stdio adapter', async (t) => {
  try {
    await new Promise((resolve, reject) => {
      const probe = spawn('gleam', ['--version'], { stdio: 'ignore' });
      probe.once('error', reject);
      probe.once('close', (status) => status === 0 ? resolve() : reject(new Error('gleam unavailable')));
    });
  } catch {
    t.skip('Gleam executable is unavailable on this host');
    return;
  }
  const result = await invokePolyglot(
    'gleam',
    `pub fn handle(request_json: String, _context_json: String) -> String {
  request_json
}`,
    { value: 42 },
  );
  assert.deepEqual(result, { value: 42 });
});
